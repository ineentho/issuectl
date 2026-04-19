use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::error::{CliError, CliResult};

const MIGRATIONS: &[(&str, &str)] = &[("0001_init", include_str!("../migrations/0001_init.sql"))];
const WRITE_LOCK_NAME: &str = "global_write";

pub fn resolve_db_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("ISSUECLI_DB_PATH") {
        return Ok(PathBuf::from(path));
    }

    let home = dirs::home_dir().context("failed to resolve home directory")?;
    Ok(home.join(".issuecli").join("db.sqlite3"))
}

pub fn initialize_database(db_path: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let conn = open_connection(db_path)?;
    ensure_migrations(&conn)
}

pub fn open_connection(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("failed to open database at {}", db_path.display()))?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

fn ensure_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );",
    )?;

    for (version, sql) in MIGRATIONS {
        let already_applied: Option<String> = conn
            .query_row(
                "SELECT version FROM schema_migrations WHERE version = ?1",
                params![version],
                |row| row.get(0),
            )
            .optional()?;

        if already_applied.is_none() {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![version, now_string()],
            )?;
            tx.commit()?;
        }
    }

    Ok(())
}

pub fn now_string() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    format!("{}.{:09}Z", now.as_secs(), now.subsec_nanos())
}

pub fn owner_id() -> String {
    format!("{}-{}", std::process::id(), now_string())
}

fn lease_until() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    let later = now + Duration::from_secs(30);
    format!("{}.{:09}Z", later.as_secs(), later.subsec_nanos())
}

pub fn with_write<T, F>(conn: &mut Connection, owner_id: &str, mut f: F) -> CliResult<T>
where
    F: FnMut(&Transaction<'_>) -> CliResult<T>,
{
    let tx = conn.transaction().map_err(anyhow::Error::from)?;
    acquire_lock(&tx, owner_id)?;
    let result = f(&tx);
    match result {
        Ok(value) => {
            release_lock(&tx, owner_id)?;
            tx.commit().map_err(anyhow::Error::from)?;
            Ok(value)
        }
        Err(err) => Err(err),
    }
}

fn acquire_lock(tx: &Transaction<'_>, owner_id: &str) -> CliResult<()> {
    tx.execute(
        "DELETE FROM locks WHERE lock_name = ?1 AND leased_until < ?2",
        params![WRITE_LOCK_NAME, now_string()],
    )?;
    let updated = tx.execute(
        "INSERT INTO locks (lock_name, owner_id, leased_until, heartbeat_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(lock_name) DO UPDATE SET owner_id=excluded.owner_id, leased_until=excluded.leased_until, heartbeat_at=excluded.heartbeat_at
         WHERE locks.leased_until < excluded.heartbeat_at OR locks.owner_id = excluded.owner_id",
        params![WRITE_LOCK_NAME, owner_id, lease_until(), now_string()],
    )?;
    if updated == 0 {
        return Err(CliError::Operational(anyhow::anyhow!(
            "another writer currently holds the issuecli lease"
        )));
    }
    Ok(())
}

fn release_lock(tx: &Transaction<'_>, owner_id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM locks WHERE lock_name = ?1 AND owner_id = ?2",
        params![WRITE_LOCK_NAME, owner_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    use super::*;
    use crate::error::validation;

    #[test]
    fn initialize_database_is_idempotent_and_creates_schema() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("db.sqlite3");

        initialize_database(&db_path).unwrap();
        initialize_database(&db_path).unwrap();

        let conn = open_connection(&db_path).unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('projects', 'work_items', 'commands', 'events', 'locks')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 5);
    }

    #[test]
    fn initialize_database_applies_migration_to_partial_database() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("db.sqlite3");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .unwrap();
        drop(conn);

        initialize_database(&db_path).unwrap();
        let conn = open_connection(&db_path).unwrap();
        let has_work_items: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='work_items'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_work_items, 1);
    }

    #[test]
    fn with_write_rejects_existing_live_lock() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("db.sqlite3");
        initialize_database(&db_path).unwrap();

        let conn = open_connection(&db_path).unwrap();
        conn.execute(
            "INSERT INTO locks (lock_name, owner_id, leased_until, heartbeat_at) VALUES (?1, ?2, ?3, ?4)",
            params![WRITE_LOCK_NAME, "someone-else", lease_until(), now_string()],
        )
        .unwrap();
        drop(conn);

        let mut conn = open_connection(&db_path).unwrap();
        let err = with_write(&mut conn, "me", |_tx| Ok(())).unwrap_err();
        assert!(
            err.to_string()
                .contains("writer currently holds the issuecli lease")
        );
    }

    #[test]
    fn with_write_rolls_back_error_and_releases_lock() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("db.sqlite3");
        initialize_database(&db_path).unwrap();

        let mut conn = open_connection(&db_path).unwrap();
        let err = with_write(&mut conn, "writer-1", |tx| {
            tx.execute("INSERT INTO metadata (key, value) VALUES ('temp', '1')", [])
                .unwrap();
            validation::<()>("fail")
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "fail");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata WHERE key='temp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);

        with_write(&mut conn, "writer-2", |tx| {
            tx.execute("INSERT INTO metadata (key, value) VALUES ('temp', '2')", [])
                .unwrap();
            Ok(())
        })
        .unwrap();

        let count_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata WHERE key='temp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after, 1);
    }
}

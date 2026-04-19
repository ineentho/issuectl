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

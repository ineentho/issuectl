use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::db::{now_string, open_connection, owner_id, resolve_db_path, with_write};
use crate::domain::{
    CommandRecord, EventRecord, InternalCommandRecord, ItemListFilter, ProjectRecord, ProjectRow,
    TreeNode, WorkItemRecord, WorkItemRow, bool_to_i64, parse_json, project_from_value,
    work_item_event_key, work_item_from_value,
};
use crate::error::{CliError, CliResult, validation};
use crate::git::find_repo_root;

#[derive(Debug, Clone, Serialize)]
pub struct ProjectResolution {
    pub project: ProjectRecord,
    pub source: &'static str,
    pub repo_root: Option<String>,
    pub override_project_id: Option<String>,
    pub created: bool,
}

pub fn resolve_active_project(
    conn: &mut Connection,
    create_if_missing: bool,
    json: bool,
) -> CliResult<ProjectRecord> {
    Ok(resolve_active_project_resolution(conn, create_if_missing, json)?.project)
}

pub fn resolve_active_project_readonly(
    conn: &Connection,
    create_if_missing: bool,
    json: bool,
) -> CliResult<ProjectRow> {
    let resolution = resolve_active_project_resolution(conn, create_if_missing, json)?;
    find_project_by_public_id(conn, &resolution.project.public_id)?
        .context("failed to resolve project")
        .map_err(CliError::Operational)
}

pub fn resolve_active_project_resolution(
    conn: &Connection,
    create_if_missing: bool,
    json: bool,
) -> CliResult<ProjectResolution> {
    let repo_root = find_repo_root().map(|path| path.to_string_lossy().to_string());

    if let Some(project) = find_project_override(conn)? {
        return Ok(ProjectResolution {
            override_project_id: Some(project.record.public_id.clone()),
            repo_root,
            source: "project_override",
            created: false,
            project: project.record,
        });
    }

    if let Some(repo_root_string) = &repo_root {
        if let Some(project) = find_project_by_repo_root(conn, repo_root_string)? {
            return Ok(ProjectResolution {
                override_project_id: None,
                repo_root,
                source: "repo_root",
                created: false,
                project: project.record,
            });
        }
        if create_if_missing {
            let mut conn = open_connection(&resolve_db_path()?)?;
            let owner = owner_id();
            let record = with_write(&mut conn, &owner, |tx| {
                get_or_create_project(tx, Path::new(repo_root_string), true)
            })?;
            return Ok(ProjectResolution {
                override_project_id: None,
                repo_root,
                source: "repo_root",
                created: true,
                project: record,
            });
        }
    }

    Err(CliError::Validation {
        message: "no active project found; run issuectl init inside a Git repository or use issuectl project use <project-id>".to_string(),
        json,
    })
}

pub fn resolve_active_project_with_override(
    conn: &mut Connection,
    override_project_id: Option<&str>,
    json: bool,
) -> CliResult<ProjectRow> {
    if let Some(project_id) = override_project_id {
        return find_project_by_public_id(conn, project_id)?.ok_or_else(|| CliError::Validation {
            message: format!("unknown project id: {project_id}"),
            json,
        });
    }

    resolve_active_project_readonly(conn, false, json)
}

pub fn resolve_project_tx_with_override(
    tx: &Transaction<'_>,
    override_project_id: Option<&str>,
    create_if_missing: bool,
    json: bool,
) -> CliResult<ProjectRow> {
    if let Some(project_id) = override_project_id {
        return find_project_by_public_id_tx(tx, project_id)?.ok_or_else(|| CliError::Validation {
            message: format!("unknown project id: {project_id}"),
            json,
        });
    }

    resolve_project_tx(tx, create_if_missing)
}

pub fn resolve_project_tx(tx: &Transaction<'_>, create_if_missing: bool) -> CliResult<ProjectRow> {
    if let Some(project) = find_project_override_tx(tx)? {
        return Ok(project);
    }

    if let Some(repo_root) = find_repo_root() {
        let repo_root_string = repo_root.to_string_lossy().to_string();
        if let Some(project) = find_project_by_repo_root_tx(tx, &repo_root_string)? {
            return Ok(project);
        }
        if create_if_missing {
            let record = get_or_create_project(tx, &repo_root, false)?;
            return find_project_by_public_id_tx(tx, &record.public_id)?
                .context("failed to resolve created project")
                .map_err(CliError::Operational);
        }
    }

    Err(CliError::Validation {
        message: "no active project found; run issuectl init inside a Git repository or use issuectl project use <project-id>".to_string(),
        json: false,
    })
}

pub fn resolve_active_item(
    conn: &Connection,
    item_id: &str,
    json: bool,
) -> CliResult<(ProjectRow, WorkItemRow)> {
    let project = resolve_active_project_readonly(conn, false, json)?;
    let item = get_item_by_public_id_readonly(conn, project.id, item_id)?;
    Ok((project, item))
}

pub fn get_or_create_project(
    tx: &Transaction<'_>,
    repo_root: &Path,
    record_event: bool,
) -> CliResult<ProjectRecord> {
    let repo_root_string = repo_root.to_string_lossy().to_string();
    if let Some(project) = find_project_by_repo_root_tx(tx, &repo_root_string)? {
        return Ok(project.record);
    }

    let now = now_string();
    let id_num = allocate_sequence(tx, "project_seq")?;
    let public_id = format!("PRJ-{id_num}");
    let name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_string();
    let item_prefix = default_item_prefix(&name);
    tx.execute(
        "INSERT INTO projects (public_id, name, repo_root, item_prefix, created_at, updated_at, version, next_item_number) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 1)",
        params![public_id, name, repo_root_string, item_prefix, now, now],
    )?;
    let created = find_project_by_repo_root_tx(tx, &repo_root_string)?
        .context("failed to read created project")?;
    if record_event {
        let command = create_command(tx, Some(created.id), "project.init", None)?;
        insert_event(
            tx,
            command.id,
            Some(created.id),
            "project",
            &created.record.public_id,
            "create",
            None,
            Some(json!(created.record)),
        )?;
    }
    Ok(created.record)
}

pub fn list_projects(conn: &Connection) -> Result<Vec<ProjectRecord>> {
    let mut stmt = conn.prepare(
        "SELECT public_id, name, repo_root, item_prefix, version, created_at, updated_at FROM projects ORDER BY updated_at DESC, public_id ASC",
    )?;
    let rows = stmt.query_map([], |row| Ok(project_from_row(row)))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn update_project_prefix(
    tx: &Transaction<'_>,
    project_id: &str,
    prefix: &str,
) -> CliResult<ProjectRecord> {
    let normalized = normalize_item_prefix(prefix)?;
    let project =
        find_project_by_public_id_tx(tx, project_id)?.ok_or_else(|| CliError::Validation {
            message: format!("unknown project id: {project_id}"),
            json: false,
        })?;
    let before = json!(project.record);
    let updated_at = now_string();
    let version = project.record.version + 1;
    tx.execute(
        "UPDATE projects SET item_prefix = ?1, updated_at = ?2, version = ?3 WHERE id = ?4",
        params![normalized, updated_at, version, project.id],
    )?;
    let updated =
        find_project_by_public_id_tx(tx, project_id)?.context("failed to read updated project")?;
    let command = create_command(tx, Some(project.id), "project.update", None)?;
    insert_event(
        tx,
        command.id,
        Some(project.id),
        "project",
        &updated.record.public_id,
        "update",
        Some(before),
        Some(json!(updated.record)),
    )?;
    Ok(updated.record)
}

pub fn set_project_override(tx: &Transaction<'_>, project_id: &str) -> CliResult<ProjectRecord> {
    let project =
        find_project_by_public_id_tx(tx, project_id)?.ok_or_else(|| CliError::Validation {
            message: format!("unknown project id: {project_id}"),
            json: false,
        })?;
    tx.execute(
        "INSERT INTO project_overrides (scope, project_id) VALUES ('global', ?1)
         ON CONFLICT(scope) DO UPDATE SET project_id=excluded.project_id",
        params![project.id],
    )?;
    let command = create_command(tx, Some(project.id), "project.use", None)?;
    insert_event(
        tx,
        command.id,
        Some(project.id),
        "project_override",
        "global",
        "set",
        None,
        Some(json!({ "project_id": project.record.public_id })),
    )?;
    Ok(project.record)
}

pub fn allocate_project_item_number(tx: &Transaction<'_>, project_id: i64) -> Result<i64> {
    let current: i64 = tx.query_row(
        "SELECT next_item_number FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "UPDATE projects SET next_item_number = next_item_number + 1, updated_at = ?2, version = version + 1 WHERE id = ?1",
        params![project_id, now_string()],
    )?;
    Ok(current)
}

pub fn create_command(
    tx: &Transaction<'_>,
    project_id: Option<i64>,
    action: &str,
    undone_command_id: Option<i64>,
) -> Result<InternalCommandRecord> {
    let seq = allocate_sequence(tx, "command_seq")?;
    let public_id = format!("CMD-{seq}");
    let now = now_string();
    tx.execute(
        "INSERT INTO commands (public_id, project_id, action, actor, undone_command_id, created_at) VALUES (?1, ?2, ?3, 'issuectl', ?4, ?5)",
        params![public_id, project_id, action, undone_command_id, now],
    )?;
    Ok(InternalCommandRecord {
        id: tx.last_insert_rowid(),
        public_id,
        project_id,
        action: action.to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn insert_event(
    tx: &Transaction<'_>,
    command_id: i64,
    project_id: Option<i64>,
    entity_type: &str,
    entity_key: &str,
    operation: &str,
    before_state: Option<Value>,
    after_state: Option<Value>,
) -> Result<()> {
    tx.execute(
        "INSERT INTO events (command_id, project_id, entity_type, entity_key, operation, before_state, after_state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![command_id, project_id, entity_type, entity_key, operation, before_state.map(|value| value.to_string()), after_state.map(|value| value.to_string()), now_string()],
    )?;
    Ok(())
}

pub fn get_item_by_public_id(
    tx: &Transaction<'_>,
    project_id: i64,
    item_id: &str,
) -> CliResult<WorkItemRow> {
    get_item_by_public_id_tx(tx, project_id, item_id)?.ok_or_else(|| CliError::Validation {
        message: format!("unknown item id: {item_id}"),
        json: false,
    })
}

pub fn get_item_by_public_id_readonly(
    conn: &Connection,
    project_id: i64,
    item_id: &str,
) -> CliResult<WorkItemRow> {
    conn.query_row(
        "SELECT wi.id, wi.public_id, wi.project_id, wi.title, wi.description, wi.ready, wi.status, wi.priority,
                parent.public_id, wi.created_at, wi.updated_at, wi.closed_at, wi.version
         FROM work_items wi
         LEFT JOIN work_items parent ON parent.id = wi.parent_id
         WHERE wi.project_id = ?1 AND wi.public_id = ?2",
        params![project_id, item_id],
        |row| Ok(work_item_row_from_row(row)),
    )
    .optional()?
    .ok_or_else(|| CliError::Validation { message: format!("unknown item id: {item_id}"), json: false })
}

pub fn resolve_parent_row_id(
    tx: &Transaction<'_>,
    project_id: i64,
    parent_id: Option<&str>,
) -> CliResult<Option<i64>> {
    match parent_id {
        Some(parent_id) => Ok(Some(
            get_item_by_public_id(tx, project_id, parent_id)?.row_id,
        )),
        None => Ok(None),
    }
}

pub fn ensure_valid_parent(
    tx: &Transaction<'_>,
    project_id: i64,
    child_row_id: i64,
    parent_row_id: Option<i64>,
) -> CliResult<()> {
    if let Some(parent_row_id) = parent_row_id {
        if child_row_id == parent_row_id {
            return validation("an item cannot be its own parent");
        }

        let mut current = Some(parent_row_id);
        while let Some(row_id) = current {
            if row_id == child_row_id {
                return validation("parent relationship would create a cycle");
            }
            current = tx
                .query_row(
                    "SELECT parent_id FROM work_items WHERE id = ?1 AND project_id = ?2",
                    params![row_id, project_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
        }
    }

    Ok(())
}

pub fn ensure_no_block_cycle(
    tx: &Transaction<'_>,
    blocker_row_id: i64,
    blocked_row_id: i64,
) -> CliResult<()> {
    if path_exists(tx, blocked_row_id, blocker_row_id)? {
        return validation("blocking relationship would create a cycle");
    }
    Ok(())
}

pub fn list_items(
    conn: &Connection,
    project_id: i64,
    filters: &ItemListFilter,
) -> Result<Vec<WorkItemRecord>> {
    let mut stmt = conn.prepare(
        "SELECT wi.id, wi.public_id, wi.project_id, wi.title, wi.description, wi.ready, wi.status, wi.priority,
                parent.public_id, wi.created_at, wi.updated_at, wi.closed_at, wi.version
         FROM work_items wi
         LEFT JOIN work_items parent ON parent.id = wi.parent_id
         WHERE wi.project_id = ?1
         ORDER BY CASE wi.priority WHEN 'urgent' THEN 4 WHEN 'high' THEN 3 WHEN 'medium' THEN 2 ELSE 1 END DESC,
                  wi.created_at ASC,
                  wi.public_id ASC",
    )?;

    let rows = stmt.query_map(params![project_id], |row| {
        Ok(work_item_row_from_row(row).record)
    })?;
    let all = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    let filtered = all
        .into_iter()
        .filter(|item| {
            filters
                .status
                .is_none_or(|status| item.status == status.to_string())
        })
        .filter(|item| {
            filters
                .priority
                .is_none_or(|priority| item.priority == priority.to_string())
        })
        .filter(|item| filters.ready.is_none_or(|ready| item.ready == ready))
        .filter(|item| {
            if filters.root {
                item.parent_id.is_none()
            } else {
                true
            }
        })
        .filter(|item| {
            filters
                .parent
                .as_ref()
                .is_none_or(|parent| item.parent_id.as_ref() == Some(parent))
        })
        .collect::<Vec<_>>();

    if let Some(blocked) = filters.blocked {
        let mut result = Vec::with_capacity(filtered.len());
        for item in filtered {
            let is_blocked = has_active_blocker(conn, project_id, &item.public_id)?;
            if is_blocked == blocked {
                result.push(item);
            }
        }
        Ok(result)
    } else {
        Ok(filtered)
    }
}

pub fn list_children(
    conn: &Connection,
    project_id: i64,
    parent_row_id: i64,
) -> Result<Vec<WorkItemRecord>> {
    let mut stmt = conn.prepare(
        "SELECT wi.id, wi.public_id, wi.project_id, wi.title, wi.description, wi.ready, wi.status, wi.priority,
                parent.public_id, wi.created_at, wi.updated_at, wi.closed_at, wi.version
         FROM work_items wi
         LEFT JOIN work_items parent ON parent.id = wi.parent_id
         WHERE wi.project_id = ?1 AND wi.parent_id = ?2
         ORDER BY wi.created_at ASC, wi.public_id ASC",
    )?;
    let rows = stmt.query_map(params![project_id, parent_row_id], |row| {
        Ok(work_item_row_from_row(row).record)
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn list_root_items(conn: &Connection, project_id: i64) -> Result<Vec<WorkItemRecord>> {
    let mut stmt = conn.prepare(
        "SELECT wi.id, wi.public_id, wi.project_id, wi.title, wi.description, wi.ready, wi.status, wi.priority,
                parent.public_id, wi.created_at, wi.updated_at, wi.closed_at, wi.version
         FROM work_items wi
         LEFT JOIN work_items parent ON parent.id = wi.parent_id
         WHERE wi.project_id = ?1 AND wi.parent_id IS NULL
         ORDER BY wi.created_at ASC, wi.public_id ASC",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(work_item_row_from_row(row).record)
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn build_tree(conn: &Connection, project_id: i64, item: WorkItemRecord) -> Result<TreeNode> {
    let row = get_item_by_public_id_readonly(conn, project_id, &item.public_id)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let children = list_children(conn, project_id, row.row_id)?
        .into_iter()
        .map(|child| build_tree(conn, project_id, child))
        .collect::<Result<Vec<_>>>()?;
    Ok(TreeNode { item, children })
}

pub fn list_blockers(conn: &Connection, project_id: i64, item_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT blocker.public_id
         FROM work_item_blockers rel
         JOIN work_items blocked ON blocked.id = rel.blocked_id
         JOIN work_items blocker ON blocker.id = rel.blocker_id
         WHERE blocked.project_id = ?1 AND blocked.public_id = ?2
         ORDER BY blocker.public_id ASC",
    )?;
    let rows = stmt.query_map(params![project_id, item_id], |row| row.get(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn list_blocked_by(conn: &Connection, project_id: i64, item_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT blocked.public_id
         FROM work_item_blockers rel
         JOIN work_items blocker ON blocker.id = rel.blocker_id
         JOIN work_items blocked ON blocked.id = rel.blocked_id
         WHERE blocker.project_id = ?1 AND blocker.public_id = ?2
         ORDER BY blocked.public_id ASC",
    )?;
    let rows = stmt.query_map(params![project_id, item_id], |row| row.get(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn select_next_items(
    conn: &Connection,
    project_id: i64,
    limit: usize,
) -> Result<Vec<WorkItemRecord>> {
    let mut stmt = conn.prepare(
        "SELECT wi.id, wi.public_id, wi.project_id, wi.title, wi.description, wi.ready, wi.status, wi.priority,
                parent.public_id, wi.created_at, wi.updated_at, wi.closed_at, wi.version
         FROM work_items wi
         LEFT JOIN work_items parent ON parent.id = wi.parent_id
         WHERE wi.project_id = ?1
           AND wi.ready = 1
           AND wi.status NOT IN ('done', 'cancelled')
           AND NOT EXISTS (
               SELECT 1
               FROM work_item_blockers rel
               JOIN work_items blocker ON blocker.id = rel.blocker_id
               WHERE rel.blocked_id = wi.id AND blocker.status NOT IN ('done', 'cancelled')
           )
           AND NOT EXISTS (
               SELECT 1
               FROM work_items child
               WHERE child.parent_id = wi.id AND child.status NOT IN ('done', 'cancelled')
           )
         ORDER BY CASE wi.priority WHEN 'urgent' THEN 4 WHEN 'high' THEN 3 WHEN 'medium' THEN 2 ELSE 1 END DESC,
                  wi.updated_at ASC,
                  wi.created_at ASC,
                  wi.public_id ASC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project_id, limit as i64], |row| {
        Ok(work_item_row_from_row(row).record)
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn list_item_history(
    conn: &Connection,
    project_id: i64,
    item_id: &str,
) -> Result<Vec<EventRecord>> {
    let mut stmt = conn.prepare(
        "SELECT entity_type, entity_key, operation, before_state, after_state, created_at
         FROM events
         WHERE project_id = ?1 AND entity_type = 'work_item' AND entity_key = ?2
         ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(
        params![project_id, work_item_event_key(project_id, item_id)],
        |row| Ok(event_from_row(row)),
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn get_command_history(
    conn: &Connection,
    command_id: &str,
) -> CliResult<(CommandRecord, Vec<EventRecord>)> {
    let command = conn
        .query_row(
            "SELECT c.public_id, p.public_id, c.action, c.actor, undone.public_id, c.created_at
             FROM commands c
             LEFT JOIN projects p ON p.id = c.project_id
             LEFT JOIN commands undone ON undone.id = c.undone_command_id
             WHERE c.public_id = ?1",
            params![command_id],
            |row| {
                Ok(CommandRecord {
                    public_id: row.get(0)?,
                    project_id: row.get(1)?,
                    action: row.get(2)?,
                    actor: row.get(3)?,
                    undone_command_id: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| CliError::Validation {
            message: format!("unknown command id: {command_id}"),
            json: false,
        })?;

    let mut stmt = conn.prepare(
        "SELECT entity_type, entity_key, operation, before_state, after_state, e.created_at
         FROM events e
         JOIN commands c ON c.id = e.command_id
         WHERE c.public_id = ?1
         ORDER BY e.id ASC",
    )?;
    let rows = stmt.query_map(params![command_id], |row| Ok(event_from_row(row)))?;
    let events = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((command, events))
}

pub fn list_commands(conn: &Connection, project_id: i64) -> Result<Vec<CommandRecord>> {
    let mut stmt = conn.prepare(
        "SELECT c.public_id, p.public_id, c.action, c.actor, undone.public_id, c.created_at
         FROM commands c
         LEFT JOIN projects p ON p.id = c.project_id
         LEFT JOIN commands undone ON undone.id = c.undone_command_id
         WHERE c.project_id = ?1
         ORDER BY c.id DESC
         LIMIT 50",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(CommandRecord {
            public_id: row.get(0)?,
            project_id: row.get(1)?,
            action: row.get(2)?,
            actor: row.get(3)?,
            undone_command_id: row.get(4)?,
            created_at: row.get(5)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn undo_command(tx: &Transaction<'_>, command_id: &str) -> CliResult<Value> {
    let original = tx.query_row(
        "SELECT id, public_id, project_id, action, actor, undone_command_id, created_at FROM commands WHERE public_id = ?1",
        params![command_id],
        |row| {
            Ok(InternalCommandRecord {
                id: row.get(0)?,
                public_id: row.get(1)?,
                project_id: row.get(2)?,
                action: row.get(3)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| CliError::Validation { message: format!("unknown command id: {command_id}"), json: false })?;

    let mut stmt = tx.prepare(
        "SELECT id, entity_type, entity_key, operation, before_state, after_state, created_at
         FROM events WHERE command_id = ?1 ORDER BY id DESC",
    )?;
    let rows = stmt.query_map(params![original.id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    let events = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    if events.is_empty() {
        return validation("command has no reversible events");
    }

    for (_, entity_type, entity_key, _, _, _, _) in &events {
        let has_later_change: Option<i64> = tx.query_row(
            "SELECT id FROM events WHERE entity_type = ?1 AND entity_key = ?2 AND command_id != ?3 AND id > (SELECT MAX(id) FROM events WHERE command_id = ?3 AND entity_type = ?1 AND entity_key = ?2)",
            params![entity_type, entity_key, original.id],
            |row| row.get(0),
        ).optional()?;
        if has_later_change.is_some() {
            return validation(&format!(
                "cannot undo {command_id}: later changes exist for {entity_type} {entity_key}"
            ));
        }
    }

    let undo = create_command(
        tx,
        original.project_id,
        &format!("undo:{}", original.action),
        Some(original.id),
    )?;
    let mut reversed = Vec::new();
    for (_, entity_type, entity_key, operation, before_state, after_state, _) in events {
        match entity_type.as_str() {
            "work_item" => {
                apply_item_undo(
                    tx,
                    original.project_id,
                    &entity_key,
                    before_state.as_deref(),
                    after_state.as_deref(),
                )?;
                insert_event(
                    tx,
                    undo.id,
                    original.project_id,
                    "work_item",
                    &entity_key,
                    &format!("undo_{operation}"),
                    after_state.as_deref().and_then(parse_json),
                    before_state.as_deref().and_then(parse_json),
                )?;
            }
            "blocker_relation" => {
                apply_relation_undo(
                    tx,
                    original.project_id,
                    before_state.as_deref(),
                    after_state.as_deref(),
                )?;
                insert_event(
                    tx,
                    undo.id,
                    original.project_id,
                    "blocker_relation",
                    &entity_key,
                    &format!("undo_{operation}"),
                    after_state.as_deref().and_then(parse_json),
                    before_state.as_deref().and_then(parse_json),
                )?;
            }
            "project_override" => {
                if let Some(before) = before_state.as_deref().and_then(parse_json) {
                    let project_public_id = before
                        .get("project_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let project = find_project_by_public_id_tx(tx, project_public_id)?
                        .context("unknown project for override restore")?;
                    tx.execute(
                        "INSERT INTO project_overrides (scope, project_id) VALUES ('global', ?1)
                         ON CONFLICT(scope) DO UPDATE SET project_id=excluded.project_id",
                        params![project.id],
                    )?;
                } else {
                    tx.execute("DELETE FROM project_overrides WHERE scope='global'", [])?;
                }
                insert_event(
                    tx,
                    undo.id,
                    original.project_id,
                    "project_override",
                    &entity_key,
                    &format!("undo_{operation}"),
                    after_state.as_deref().and_then(parse_json),
                    before_state.as_deref().and_then(parse_json),
                )?;
            }
            "project" => {
                if operation == "create" {
                    return validation("undo for project creation is not supported in the MVP");
                }
                apply_project_undo(tx, &entity_key, before_state.as_deref())?;
                insert_event(
                    tx,
                    undo.id,
                    original.project_id,
                    "project",
                    &entity_key,
                    &format!("undo_{operation}"),
                    after_state.as_deref().and_then(parse_json),
                    before_state.as_deref().and_then(parse_json),
                )?;
            }
            _ => return validation(&format!("unsupported undo entity type: {entity_type}")),
        }
        reversed.push(entity_key);
    }

    Ok(json!({ "command": undo.public_id, "reversed_command": command_id, "entities": reversed }))
}

fn find_project_by_repo_root(conn: &Connection, repo_root: &str) -> Result<Option<ProjectRow>> {
    conn.query_row(
        "SELECT id, public_id, name, repo_root, item_prefix, version, created_at, updated_at FROM projects WHERE repo_root = ?1",
        params![repo_root],
        |row| Ok(ProjectRow { id: row.get(0)?, record: project_from_row_offset(row, 1) }),
    )
    .optional()
    .map_err(Into::into)
}

fn apply_project_undo(
    tx: &Transaction<'_>,
    project_id: &str,
    before_state: Option<&str>,
) -> CliResult<()> {
    let before = before_state
        .and_then(parse_json)
        .ok_or_else(|| CliError::Operational(anyhow::anyhow!("missing project state for undo")))?;
    let project = project_from_value(&before).map_err(CliError::Operational)?;
    tx.execute(
        "UPDATE projects SET name = ?1, repo_root = ?2, item_prefix = ?3, updated_at = ?4, version = ?5 WHERE public_id = ?6",
        params![project.name, project.repo_root, project.item_prefix, project.updated_at, project.version, project_id],
    )?;
    Ok(())
}

fn find_project_by_repo_root_tx(
    tx: &Transaction<'_>,
    repo_root: &str,
) -> Result<Option<ProjectRow>> {
    tx.query_row(
        "SELECT id, public_id, name, repo_root, item_prefix, version, created_at, updated_at FROM projects WHERE repo_root = ?1",
        params![repo_root],
        |row| Ok(ProjectRow { id: row.get(0)?, record: project_from_row_offset(row, 1) }),
    )
    .optional()
    .map_err(Into::into)
}

fn find_project_by_public_id(conn: &Connection, project_id: &str) -> Result<Option<ProjectRow>> {
    conn.query_row(
        "SELECT id, public_id, name, repo_root, item_prefix, version, created_at, updated_at FROM projects WHERE public_id = ?1",
        params![project_id],
        |row| Ok(ProjectRow { id: row.get(0)?, record: project_from_row_offset(row, 1) }),
    )
    .optional()
    .map_err(Into::into)
}

fn find_project_by_public_id_tx(
    tx: &Transaction<'_>,
    project_id: &str,
) -> Result<Option<ProjectRow>> {
    tx.query_row(
        "SELECT id, public_id, name, repo_root, item_prefix, version, created_at, updated_at FROM projects WHERE public_id = ?1",
        params![project_id],
        |row| Ok(ProjectRow { id: row.get(0)?, record: project_from_row_offset(row, 1) }),
    )
    .optional()
    .map_err(Into::into)
}

fn find_project_override(conn: &Connection) -> Result<Option<ProjectRow>> {
    conn.query_row(
        "SELECT p.id, p.public_id, p.name, p.repo_root, p.item_prefix, p.version, p.created_at, p.updated_at
         FROM project_overrides po JOIN projects p ON p.id = po.project_id WHERE po.scope = 'global'",
        [],
        |row| Ok(ProjectRow { id: row.get(0)?, record: project_from_row_offset(row, 1) }),
    )
    .optional()
    .map_err(Into::into)
}

fn find_project_override_tx(tx: &Transaction<'_>) -> Result<Option<ProjectRow>> {
    tx.query_row(
        "SELECT p.id, p.public_id, p.name, p.repo_root, p.item_prefix, p.version, p.created_at, p.updated_at
         FROM project_overrides po JOIN projects p ON p.id = po.project_id WHERE po.scope = 'global'",
        [],
        |row| Ok(ProjectRow { id: row.get(0)?, record: project_from_row_offset(row, 1) }),
    )
    .optional()
    .map_err(Into::into)
}

fn allocate_sequence(tx: &Transaction<'_>, key: &str) -> Result<i64> {
    let current: Option<String> = tx
        .query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    let next = current
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        + 1;
    tx.execute(
        "INSERT INTO metadata (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, next.to_string()],
    )?;
    Ok(next)
}

fn get_item_by_public_id_tx(
    tx: &Transaction<'_>,
    project_id: i64,
    item_id: &str,
) -> Result<Option<WorkItemRow>> {
    tx.query_row(
        "SELECT wi.id, wi.public_id, wi.project_id, wi.title, wi.description, wi.ready, wi.status, wi.priority,
                parent.public_id, wi.created_at, wi.updated_at, wi.closed_at, wi.version
         FROM work_items wi
         LEFT JOIN work_items parent ON parent.id = wi.parent_id
         WHERE wi.project_id = ?1 AND wi.public_id = ?2",
        params![project_id, item_id],
        |row| Ok(work_item_row_from_row(row)),
    )
    .optional()
    .map_err(Into::into)
}

fn work_item_row_from_row(row: &rusqlite::Row<'_>) -> WorkItemRow {
    WorkItemRow {
        row_id: row.get(0).unwrap(),
        record: WorkItemRecord {
            public_id: row.get(1).unwrap(),
            project_id: row.get(2).unwrap(),
            title: row.get(3).unwrap(),
            description: row.get(4).unwrap(),
            ready: row.get::<_, i64>(5).unwrap() != 0,
            status: row.get(6).unwrap(),
            priority: row.get(7).unwrap(),
            parent_id: row.get(8).unwrap(),
            created_at: row.get(9).unwrap(),
            updated_at: row.get(10).unwrap(),
            closed_at: row.get(11).unwrap(),
            version: row.get(12).unwrap(),
        },
    }
}

fn has_active_blocker(conn: &Connection, project_id: i64, item_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM work_item_blockers rel
         JOIN work_items blocked ON blocked.id = rel.blocked_id
         JOIN work_items blocker ON blocker.id = rel.blocker_id
         WHERE blocked.project_id = ?1 AND blocked.public_id = ?2 AND blocker.status NOT IN ('done', 'cancelled')",
        params![project_id, item_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn path_exists(tx: &Transaction<'_>, start_row_id: i64, target_row_id: i64) -> Result<bool> {
    let mut stack = vec![start_row_id];
    let mut seen = HashSet::new();
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        if current == target_row_id {
            return Ok(true);
        }
        let mut stmt =
            tx.prepare("SELECT blocked_id FROM work_item_blockers WHERE blocker_id = ?1")?;
        let rows = stmt.query_map(params![current], |row| row.get::<_, i64>(0))?;
        for row in rows {
            stack.push(row?);
        }
    }
    Ok(false)
}

fn event_from_row(row: &rusqlite::Row<'_>) -> EventRecord {
    EventRecord {
        entity_type: row.get(0).unwrap(),
        entity_key: row.get(1).unwrap(),
        operation: row.get(2).unwrap(),
        before_state: row
            .get::<_, Option<String>>(3)
            .unwrap()
            .and_then(|value| serde_json::from_str(&value).ok()),
        after_state: row
            .get::<_, Option<String>>(4)
            .unwrap()
            .and_then(|value| serde_json::from_str(&value).ok()),
        created_at: row.get(5).unwrap(),
    }
}

fn apply_item_undo(
    tx: &Transaction<'_>,
    project_id: Option<i64>,
    entity_key: &str,
    before_state: Option<&str>,
    _after_state: Option<&str>,
) -> CliResult<()> {
    match before_state.and_then(parse_json) {
        Some(before) => {
            let record = work_item_from_value(&before).map_err(CliError::Operational)?;
            let project_id = project_id.unwrap_or(record.project_id);
            let existing = tx
                .query_row(
                    "SELECT id FROM work_items WHERE project_id = ?1 AND public_id = ?2",
                    params![project_id, record.public_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let parent_row_id =
                resolve_parent_row_id_any(tx, project_id, record.parent_id.as_deref())?;
            if let Some(existing_id) = existing {
                tx.execute(
                    "UPDATE work_items SET title=?1, description=?2, ready=?3, status=?4, priority=?5, parent_id=?6, created_at=?7, updated_at=?8, closed_at=?9, version=?10 WHERE id=?11",
                    params![record.title, record.description, bool_to_i64(record.ready), record.status, record.priority, parent_row_id, record.created_at, record.updated_at, record.closed_at, record.version, existing_id],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO work_items (public_id, project_id, title, description, ready, status, priority, parent_id, created_at, updated_at, closed_at, version)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![record.public_id, project_id, record.title, record.description, bool_to_i64(record.ready), record.status, record.priority, parent_row_id, record.created_at, record.updated_at, record.closed_at, record.version],
                )?;
            }
        }
        None => {
            let item_public_id = entity_key.rsplit(':').next().unwrap_or(entity_key);
            let project_id = project_id.context("missing project id for undo")?;
            tx.execute(
                "DELETE FROM work_items WHERE project_id = ?1 AND public_id = ?2",
                params![project_id, item_public_id],
            )?;
        }
    }
    Ok(())
}

fn resolve_parent_row_id_any(
    tx: &Transaction<'_>,
    project_id: i64,
    parent_id: Option<&str>,
) -> Result<Option<i64>> {
    match parent_id {
        Some(parent_id) => tx
            .query_row(
                "SELECT id FROM work_items WHERE project_id = ?1 AND public_id = ?2",
                params![project_id, parent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into),
        None => Ok(None),
    }
}

fn apply_relation_undo(
    tx: &Transaction<'_>,
    project_id: Option<i64>,
    before_state: Option<&str>,
    after_state: Option<&str>,
) -> CliResult<()> {
    let project_id = project_id.context("missing project id for relation undo")?;
    match before_state.and_then(parse_json) {
        Some(before) => {
            let blocker_id = before
                .get("blocker_id")
                .and_then(Value::as_str)
                .context("missing blocker_id")?;
            let blocked_id = before
                .get("blocked_id")
                .and_then(Value::as_str)
                .context("missing blocked_id")?;
            let blocker_row_id = tx.query_row(
                "SELECT id FROM work_items WHERE project_id = ?1 AND public_id = ?2",
                params![project_id, blocker_id],
                |row| row.get::<_, i64>(0),
            )?;
            let blocked_row_id = tx.query_row(
                "SELECT id FROM work_items WHERE project_id = ?1 AND public_id = ?2",
                params![project_id, blocked_id],
                |row| row.get::<_, i64>(0),
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO work_item_blockers (blocker_id, blocked_id, created_at) VALUES (?1, ?2, ?3)",
                params![blocker_row_id, blocked_row_id, now_string()],
            )?;
        }
        None => {
            let after = after_state
                .and_then(parse_json)
                .context("missing relation state")?;
            let blocker_id = after
                .get("blocker_id")
                .and_then(Value::as_str)
                .context("missing blocker_id")?;
            let blocked_id = after
                .get("blocked_id")
                .and_then(Value::as_str)
                .context("missing blocked_id")?;
            let blocker_row_id = tx.query_row(
                "SELECT id FROM work_items WHERE project_id = ?1 AND public_id = ?2",
                params![project_id, blocker_id],
                |row| row.get::<_, i64>(0),
            )?;
            let blocked_row_id = tx.query_row(
                "SELECT id FROM work_items WHERE project_id = ?1 AND public_id = ?2",
                params![project_id, blocked_id],
                |row| row.get::<_, i64>(0),
            )?;
            tx.execute(
                "DELETE FROM work_item_blockers WHERE blocker_id = ?1 AND blocked_id = ?2",
                params![blocker_row_id, blocked_row_id],
            )?;
        }
    }
    Ok(())
}

fn project_from_row(row: &rusqlite::Row<'_>) -> ProjectRecord {
    ProjectRecord {
        public_id: row.get::<_, String>(0).unwrap(),
        name: row.get::<_, String>(1).unwrap(),
        repo_root: row.get::<_, Option<String>>(2).unwrap(),
        item_prefix: row.get::<_, String>(3).unwrap(),
        version: row.get::<_, i64>(4).unwrap(),
        created_at: row.get::<_, String>(5).unwrap(),
        updated_at: row.get::<_, String>(6).unwrap(),
    }
}

fn project_from_row_offset(row: &rusqlite::Row<'_>, offset: usize) -> ProjectRecord {
    ProjectRecord {
        public_id: row.get(offset).unwrap(),
        name: row.get(offset + 1).unwrap(),
        repo_root: row.get(offset + 2).unwrap(),
        item_prefix: row.get(offset + 3).unwrap(),
        version: row.get(offset + 4).unwrap(),
        created_at: row.get(offset + 5).unwrap(),
        updated_at: row.get(offset + 6).unwrap(),
    }
}

fn normalize_item_prefix(prefix: &str) -> CliResult<String> {
    let normalized = prefix.trim().to_ascii_uppercase();
    if normalized.is_empty() {
        return validation("project item prefix cannot be empty");
    }
    if !normalized
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
    {
        return validation("project item prefix must use only A-Z and 0-9");
    }
    if !normalized
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        return validation("project item prefix must start with A-Z");
    }
    Ok(normalized)
}

fn default_item_prefix(name: &str) -> String {
    name.chars()
        .find(|ch| ch.is_ascii_alphabetic())
        .map(|ch| ch.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "P".to_string())
}

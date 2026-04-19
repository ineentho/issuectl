use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use crate::db::{initialize_database, now_string, open_connection, owner_id, resolve_db_path, with_write};
use crate::domain::{
    CommandRecord, EventRecord, ItemListFilter, PriorityArg, ProjectRecord, StatusArg, TreeNode,
    WorkItemRecord, blocker_relation_event_key, bool_to_i64, work_item_event_key, work_item_state,
};
use crate::error::{CliError, CliResult};
use crate::repo::{
    allocate_project_item_number, build_tree, create_command, ensure_no_block_cycle,
    ensure_valid_parent, get_command_history, get_item_by_public_id, get_item_by_public_id_readonly,
    get_or_create_project, insert_event, list_blocked_by, list_blockers, list_children,
    list_commands, list_item_history, list_items, list_projects, list_root_items,
    resolve_active_project_readonly, resolve_active_project_with_override, resolve_parent_row_id,
    resolve_project_tx, select_next_items, set_project_override, undo_command,
};

#[derive(Debug, Clone)]
pub struct OverviewSnapshot {
    pub projects: Vec<ProjectRecord>,
    pub active_project: Option<ProjectRecord>,
    pub items: Vec<WorkItemRecord>,
    pub tree: Vec<TreeNode>,
    pub next_items: Vec<WorkItemRecord>,
    pub commands: Vec<CommandRecord>,
}

#[derive(Debug, Clone)]
pub struct ItemDetail {
    pub item: WorkItemRecord,
    pub children: Vec<WorkItemRecord>,
    pub blockers: Vec<String>,
    pub blocked_by: Vec<String>,
    pub history: Vec<EventRecord>,
}

#[derive(Debug, Clone)]
pub struct CreateItemInput {
    pub title: String,
    pub description: String,
    pub priority: PriorityArg,
    pub parent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateItemInput {
    pub item_id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct IssueService {
    db_path: std::path::PathBuf,
}

impl IssueService {
    pub fn new() -> Result<Self> {
        let db_path = resolve_db_path()?;
        initialize_database(&db_path)?;
        Ok(Self { db_path })
    }

    pub fn init_current_repo_project(&self) -> CliResult<ProjectRecord> {
        let repo_root = crate::git::require_repo_root(false)?;
        let mut conn = open_connection(&self.db_path).map_err(CliError::Operational)?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| get_or_create_project(tx, &repo_root, true))
    }

    pub fn load_overview(&self, selected_project_id: Option<&str>) -> CliResult<OverviewSnapshot> {
        let projects = {
            let conn = self.open()?;
            list_projects(&conn).map_err(CliError::Operational)?
        };

        let active_project = {
            let mut conn = self.open()?;
            match selected_project_id {
                Some(project_id) => Some(
                    resolve_active_project_with_override(&mut conn, Some(project_id), false)?.record,
                ),
                None => match resolve_active_project_readonly(&conn, false, false) {
                    Ok(project) => Some(project.record),
                    Err(CliError::Validation { .. }) => None,
                    Err(err) => return Err(err),
                },
            }
        };

        let (items, tree, next_items, commands) = if let Some(project) = &active_project {
            let mut conn = self.open()?;
            let project_row = resolve_active_project_with_override(
                &mut conn,
                Some(project.public_id.as_str()),
                false,
            )?;
            let items = list_items(&conn, project_row.id, &ItemListFilter {
                status: None,
                priority: None,
                ready: None,
                blocked: None,
                parent: None,
                root: false,
            })
            .map_err(CliError::Operational)?;
            let tree = list_root_items(&conn, project_row.id)
                .map_err(CliError::Operational)?
                .into_iter()
                .map(|item| build_tree(&conn, project_row.id, item))
                .collect::<Result<Vec<_>>>()
                .map_err(CliError::Operational)?;
            let next_items = select_next_items(&conn, project_row.id, 8).map_err(CliError::Operational)?;
            let commands = list_commands(&conn, project_row.id).map_err(CliError::Operational)?;
            (items, tree, next_items, commands)
        } else {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new())
        };

        Ok(OverviewSnapshot {
            projects,
            active_project,
            items,
            tree,
            next_items,
            commands,
        })
    }

    pub fn item_detail(&self, project_id: &str, item_id: &str) -> CliResult<ItemDetail> {
        let mut conn = self.open()?;
        let project = resolve_active_project_with_override(&mut conn, Some(project_id), false)?;
        let item = get_item_by_public_id_readonly(&conn, project.id, item_id)?;
        let children = list_children(&conn, project.id, item.row_id).map_err(CliError::Operational)?;
        let blockers = list_blockers(&conn, project.id, item_id).map_err(CliError::Operational)?;
        let blocked_by = list_blocked_by(&conn, project.id, item_id).map_err(CliError::Operational)?;
        let history = list_item_history(&conn, project.id, item_id).map_err(CliError::Operational)?;
        Ok(ItemDetail {
            item: item.record,
            children,
            blockers,
            blocked_by,
            history,
        })
    }

    pub fn command_history(&self, command_id: &str) -> CliResult<(CommandRecord, Vec<EventRecord>)> {
        let conn = self.open()?;
        get_command_history(&conn, command_id)
    }

    pub fn use_project(&self, project_id: &str) -> CliResult<ProjectRecord> {
        let mut conn = self.open()?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| set_project_override(tx, project_id))
    }

    pub fn create_item(&self, input: CreateItemInput) -> CliResult<WorkItemRecord> {
        let mut conn = self.open()?;
        let owner = owner_id();
        let input = input.clone();
        with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx(tx, true)?;
            let parent = match input.parent.as_deref() {
                Some(parent_id) if !parent_id.trim().is_empty() => {
                    Some(get_item_by_public_id(tx, project.id, parent_id.trim())?)
                }
                _ => None,
            };
            let number = allocate_project_item_number(tx, project.id).map_err(CliError::Operational)?;
            let public_id = format!("WI-{number}");
            let now = now_string();
            let parent_public_id = parent.as_ref().map(|item| item.record.public_id.clone());
            let parent_row_id = parent.as_ref().map(|item| item.row_id);
            let item = WorkItemRecord {
                public_id: public_id.clone(),
                project_id: project.id,
                title: input.title.clone(),
                description: input.description.clone(),
                ready: false,
                status: StatusArg::Todo.to_string(),
                priority: input.priority.to_string(),
                parent_id: parent_public_id,
                created_at: now.clone(),
                updated_at: now,
                closed_at: None,
                version: 1,
            };
            tx.execute(
                "INSERT INTO work_items (public_id, project_id, title, description, ready, status, priority, parent_id, created_at, updated_at, closed_at, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![item.public_id, item.project_id, item.title, item.description, bool_to_i64(item.ready), item.status, item.priority, parent_row_id, item.created_at, item.updated_at, item.closed_at, item.version],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let persisted = get_item_by_public_id(tx, project.id, &public_id)?;
            let command = create_command(tx, Some(project.id), "item.create", None).map_err(CliError::Operational)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &public_id),
                "create",
                None,
                Some(work_item_state(&persisted.record)),
            )
            .map_err(CliError::Operational)?;
            Ok(persisted.record)
        })
    }

    pub fn update_item(&self, input: UpdateItemInput) -> CliResult<WorkItemRecord> {
        let mut conn = self.open()?;
        let owner = owner_id();
        let input = input.clone();
        with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx(tx, true)?;
            let existing = get_item_by_public_id(tx, project.id, &input.item_id)?;
            let before = work_item_state(&existing.record);
            let mut item = existing.record.clone();
            item.title = input.title.clone();
            item.description = input.description.clone();
            item.updated_at = now_string();
            item.version += 1;
            tx.execute(
                "UPDATE work_items SET title=?1, description=?2, updated_at=?3, version=?4 WHERE id=?5",
                rusqlite::params![item.title, item.description, item.updated_at, item.version, existing.row_id],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(tx, Some(project.id), "item.update", None).map_err(CliError::Operational)?;
            let persisted = get_item_by_public_id(tx, project.id, &input.item_id)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &item.public_id),
                "update",
                Some(before),
                Some(work_item_state(&persisted.record)),
            )
            .map_err(CliError::Operational)?;
            Ok(persisted.record)
        })
    }

    pub fn set_status(&self, item_id: &str, status: StatusArg) -> CliResult<WorkItemRecord> {
        let mut conn = self.open()?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx(tx, true)?;
            let existing = get_item_by_public_id(tx, project.id, item_id)?;
            let before = work_item_state(&existing.record);
            let mut item = existing.record.clone();
            item.status = status.to_string();
            item.closed_at = if status.is_terminal() { Some(now_string()) } else { None };
            item.updated_at = now_string();
            item.version += 1;
            tx.execute(
                "UPDATE work_items SET status=?1, updated_at=?2, closed_at=?3, version=?4 WHERE id=?5",
                rusqlite::params![item.status, item.updated_at, item.closed_at, item.version, existing.row_id],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(tx, Some(project.id), "item.update", None).map_err(CliError::Operational)?;
            let persisted = get_item_by_public_id(tx, project.id, item_id)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &item.public_id),
                "update",
                Some(before),
                Some(work_item_state(&persisted.record)),
            )
            .map_err(CliError::Operational)?;
            Ok(persisted.record)
        })
    }

    pub fn set_ready(&self, item_id: &str, ready: bool) -> CliResult<WorkItemRecord> {
        let mut conn = self.open()?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx(tx, true)?;
            let existing = get_item_by_public_id(tx, project.id, item_id)?;
            let before = work_item_state(&existing.record);
            let mut item = existing.record.clone();
            item.ready = ready;
            item.updated_at = now_string();
            item.version += 1;
            tx.execute(
                "UPDATE work_items SET ready=?1, updated_at=?2, version=?3 WHERE id=?4",
                rusqlite::params![bool_to_i64(ready), item.updated_at, item.version, existing.row_id],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(tx, Some(project.id), if ready { "item.ready" } else { "item.unready" }, None).map_err(CliError::Operational)?;
            let persisted = get_item_by_public_id(tx, project.id, item_id)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &item.public_id),
                if ready { "ready" } else { "unready" },
                Some(before),
                Some(work_item_state(&persisted.record)),
            )
            .map_err(CliError::Operational)?;
            Ok(persisted.record)
        })
    }

    pub fn set_block_relation(&self, item_id: &str, blocker_id: &str, add: bool) -> CliResult<Value> {
        let mut conn = self.open()?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx(tx, true)?;
            let blocked = get_item_by_public_id(tx, project.id, item_id)?;
            let blocker = get_item_by_public_id(tx, project.id, blocker_id)?;
            if blocked.row_id == blocker.row_id {
                return Err(CliError::Validation {
                    message: "an item cannot block itself".to_string(),
                    json: false,
                });
            }
            if add {
                ensure_no_block_cycle(tx, blocker.row_id, blocked.row_id)?;
                tx.execute(
                    "INSERT OR IGNORE INTO work_item_blockers (blocker_id, blocked_id, created_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![blocker.row_id, blocked.row_id, now_string()],
                )
                .map_err(|err| CliError::Operational(err.into()))?;
            } else {
                tx.execute(
                    "DELETE FROM work_item_blockers WHERE blocker_id=?1 AND blocked_id=?2",
                    rusqlite::params![blocker.row_id, blocked.row_id],
                )
                .map_err(|err| CliError::Operational(err.into()))?;
            }
            let command = create_command(tx, Some(project.id), if add { "item.block" } else { "item.unblock" }, None).map_err(CliError::Operational)?;
            let relation_key = blocker_relation_event_key(project.id, &blocker.record.public_id, &blocked.record.public_id);
            let state = serde_json::json!({"blocker_id": blocker.record.public_id, "blocked_id": blocked.record.public_id});
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "blocker_relation",
                &relation_key,
                if add { "create" } else { "delete" },
                if add { None } else { Some(state.clone()) },
                if add { Some(state.clone()) } else { None },
            )
            .map_err(CliError::Operational)?;
            Ok(serde_json::json!({ "blocked": blocked.record, "blocker": blocker.record, "added": add }))
        })
    }

    pub fn move_item(&self, item_id: &str, parent_id: Option<&str>) -> CliResult<WorkItemRecord> {
        let mut conn = self.open()?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx(tx, true)?;
            let existing = get_item_by_public_id(tx, project.id, item_id)?;
            let before = work_item_state(&existing.record);
            let mut item = existing.record.clone();
            if let Some(parent_id) = parent_id {
                if parent_id.trim().is_empty() {
                    item.parent_id = None;
                } else {
                    let parent = get_item_by_public_id(tx, project.id, parent_id.trim())?;
                    ensure_valid_parent(tx, project.id, existing.row_id, Some(parent.row_id))?;
                    item.parent_id = Some(parent.record.public_id);
                }
            } else {
                item.parent_id = None;
            }
            item.updated_at = now_string();
            item.version += 1;
            tx.execute(
                "UPDATE work_items SET parent_id=?1, updated_at=?2, version=?3 WHERE id=?4",
                rusqlite::params![resolve_parent_row_id(tx, project.id, item.parent_id.as_deref())?, item.updated_at, item.version, existing.row_id],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(tx, Some(project.id), "item.update", None).map_err(CliError::Operational)?;
            let persisted = get_item_by_public_id(tx, project.id, item_id)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &item.public_id),
                "update",
                Some(before),
                Some(work_item_state(&persisted.record)),
            )
            .map_err(CliError::Operational)?;
            Ok(persisted.record)
        })
    }

    pub fn undo(&self, command_id: &str) -> CliResult<Value> {
        let mut conn = self.open()?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| undo_command(tx, command_id))
    }

    fn open(&self) -> CliResult<Connection> {
        open_connection(&self.db_path).map_err(CliError::Operational)
    }
}

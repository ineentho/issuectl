use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use crate::db::{
    initialize_database, now_string, open_connection, owner_id, resolve_db_path, with_write,
};
use crate::domain::{
    CommandRecord, EventRecord, ItemListFilter, PriorityArg, ProjectRecord, StatusArg, TreeNode,
    WorkItemRecord, blocker_relation_event_key, bool_to_i64, work_item_event_key, work_item_state,
};
use crate::error::{CliError, CliResult};
use crate::repo::{
    allocate_project_item_number, build_tree, create_command, ensure_no_block_cycle,
    ensure_valid_parent, get_command_history, get_item_by_public_id,
    get_item_by_public_id_readonly, get_or_create_project, insert_event, list_blocked_by,
    list_blockers, list_children, list_commands, list_item_history, list_items, list_projects,
    list_root_items, resolve_active_project_readonly, resolve_active_project_with_override,
    resolve_parent_row_id, resolve_project_tx, select_next_items, set_project_override,
    undo_command,
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
    pub priority: PriorityArg,
}

#[derive(Debug, Clone)]
pub struct IssueService {
    db_path: std::path::PathBuf,
}

impl IssueService {
    pub fn new() -> Result<Self> {
        let db_path = resolve_db_path()?;
        Self::from_db_path(db_path)
    }

    fn from_db_path(db_path: std::path::PathBuf) -> Result<Self> {
        initialize_database(&db_path)?;
        Ok(Self { db_path })
    }

    pub fn init_current_repo_project(&self) -> CliResult<ProjectRecord> {
        let repo_root = crate::git::require_repo_root(false)?;
        let mut conn = open_connection(&self.db_path).map_err(CliError::Operational)?;
        let owner = owner_id();
        with_write(&mut conn, &owner, |tx| {
            get_or_create_project(tx, &repo_root, true)
        })
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
                    resolve_active_project_with_override(&mut conn, Some(project_id), false)?
                        .record,
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
            let items = list_items(
                &conn,
                project_row.id,
                &ItemListFilter {
                    status: None,
                    priority: None,
                    ready: None,
                    blocked: None,
                    parent: None,
                    root: false,
                },
            )
            .map_err(CliError::Operational)?;
            let tree = list_root_items(&conn, project_row.id)
                .map_err(CliError::Operational)?
                .into_iter()
                .map(|item| build_tree(&conn, project_row.id, item))
                .collect::<Result<Vec<_>>>()
                .map_err(CliError::Operational)?;
            let next_items =
                select_next_items(&conn, project_row.id, 8).map_err(CliError::Operational)?;
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
        let children =
            list_children(&conn, project.id, item.row_id).map_err(CliError::Operational)?;
        let blockers = list_blockers(&conn, project.id, item_id).map_err(CliError::Operational)?;
        let blocked_by =
            list_blocked_by(&conn, project.id, item_id).map_err(CliError::Operational)?;
        let history =
            list_item_history(&conn, project.id, item_id).map_err(CliError::Operational)?;
        Ok(ItemDetail {
            item: item.record,
            children,
            blockers,
            blocked_by,
            history,
        })
    }

    pub fn command_history(
        &self,
        command_id: &str,
    ) -> CliResult<(CommandRecord, Vec<EventRecord>)> {
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
            let number =
                allocate_project_item_number(tx, project.id).map_err(CliError::Operational)?;
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
            let command = create_command(tx, Some(project.id), "item.create", None)
                .map_err(CliError::Operational)?;
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
            item.priority = input.priority.to_string();
            item.updated_at = now_string();
            item.version += 1;
            tx.execute(
                "UPDATE work_items SET title=?1, description=?2, priority=?3, updated_at=?4, version=?5 WHERE id=?6",
                rusqlite::params![item.title, item.description, item.priority, item.updated_at, item.version, existing.row_id],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(tx, Some(project.id), "item.update", None)
                .map_err(CliError::Operational)?;
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
            item.closed_at = if status.is_terminal() {
                Some(now_string())
            } else {
                None
            };
            item.updated_at = now_string();
            item.version += 1;
            tx.execute(
                "UPDATE work_items SET status=?1, updated_at=?2, closed_at=?3, version=?4 WHERE id=?5",
                rusqlite::params![item.status, item.updated_at, item.closed_at, item.version, existing.row_id],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(tx, Some(project.id), "item.update", None)
                .map_err(CliError::Operational)?;
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
                rusqlite::params![
                    bool_to_i64(ready),
                    item.updated_at,
                    item.version,
                    existing.row_id
                ],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(
                tx,
                Some(project.id),
                if ready { "item.ready" } else { "item.unready" },
                None,
            )
            .map_err(CliError::Operational)?;
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

    pub fn set_block_relation(
        &self,
        item_id: &str,
        blocker_id: &str,
        add: bool,
    ) -> CliResult<Value> {
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
            let command = create_command(
                tx,
                Some(project.id),
                if add { "item.block" } else { "item.unblock" },
                None,
            )
            .map_err(CliError::Operational)?;
            let relation_key = blocker_relation_event_key(
                project.id,
                &blocker.record.public_id,
                &blocked.record.public_id,
            );
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
            Ok(
                serde_json::json!({ "blocked": blocked.record, "blocker": blocker.record, "added": add }),
            )
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
                rusqlite::params![
                    resolve_parent_row_id(tx, project.id, item.parent_id.as_deref())?,
                    item.updated_at,
                    item.version,
                    existing.row_id
                ],
            )
            .map_err(|err| CliError::Operational(err.into()))?;
            let command = create_command(tx, Some(project.id), "item.update", None)
                .map_err(CliError::Operational)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    fn setup_service() -> (tempfile::TempDir, IssueService) {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite3");
        let service = IssueService::from_db_path(db_path).unwrap();

        let mut conn = open_connection(&service.db_path).unwrap();
        with_write(&mut conn, &owner_id(), |tx| {
            let project = get_or_create_project(tx, temp.path(), true)?;
            set_project_override(tx, &project.public_id)?;
            Ok(())
        })
        .unwrap();

        (temp, service)
    }

    fn create_project(service: &IssueService, repo_root: &Path) -> ProjectRecord {
        fs::create_dir_all(repo_root).unwrap();
        let mut conn = open_connection(&service.db_path).unwrap();
        with_write(&mut conn, &owner_id(), |tx| {
            let project = get_or_create_project(tx, repo_root, true)?;
            set_project_override(tx, &project.public_id)
        })
        .unwrap()
    }

    fn latest_command_id(service: &IssueService) -> String {
        service
            .load_overview(None)
            .unwrap()
            .commands
            .into_iter()
            .next()
            .expect("command")
            .public_id
    }

    fn current_project(service: &IssueService) -> ProjectRecord {
        service
            .load_overview(None)
            .unwrap()
            .active_project
            .expect("active project")
    }

    fn seed_item_in_project(
        service: &IssueService,
        project: &ProjectRecord,
        title: &str,
    ) -> WorkItemRecord {
        let mut conn = open_connection(&service.db_path).unwrap();
        with_write(&mut conn, &owner_id(), |tx| {
            let project_id: i64 = tx
                .query_row(
                    "SELECT id FROM projects WHERE public_id = ?1",
                    rusqlite::params![project.public_id],
                    |row| row.get(0),
                )
                .unwrap();
            let number = allocate_project_item_number(tx, project_id).unwrap();
            let public_id = format!("WI-{number}");
            let now = now_string();
            let item = WorkItemRecord {
                public_id: public_id.clone(),
                project_id,
                title: title.to_string(),
                description: "seeded".to_string(),
                ready: false,
                status: StatusArg::Todo.to_string(),
                priority: PriorityArg::Medium.to_string(),
                parent_id: None,
                created_at: now.clone(),
                updated_at: now,
                closed_at: None,
                version: 1,
            };
            tx.execute(
                "INSERT INTO work_items (public_id, project_id, title, description, ready, status, priority, parent_id, created_at, updated_at, closed_at, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, ?11)",
                rusqlite::params![item.public_id, item.project_id, item.title, item.description, bool_to_i64(item.ready), item.status, item.priority, item.created_at, item.updated_at, item.closed_at, item.version],
            )
            .unwrap();
            Ok(item)
        })
        .unwrap()
    }

    #[test]
    fn update_item_persists_priority_changes() {
        let (_temp, service) = setup_service();

        let created = service
            .create_item(CreateItemInput {
                title: "Initial".to_string(),
                description: "Created from test".to_string(),
                priority: PriorityArg::Low,
                parent: None,
            })
            .unwrap();

        let updated = service
            .update_item(UpdateItemInput {
                item_id: created.public_id.clone(),
                title: "Updated".to_string(),
                description: "Updated from test".to_string(),
                priority: PriorityArg::Urgent,
            })
            .unwrap();

        assert_eq!(updated.priority, "urgent");

        let project = current_project(&service);
        let detail = service
            .item_detail(&project.public_id, &created.public_id)
            .unwrap();
        assert_eq!(detail.item.title, "Updated");
        assert_eq!(detail.item.description, "Updated from test");
        assert_eq!(detail.item.priority, "urgent");
    }

    #[test]
    fn overview_and_project_switching_are_project_scoped() {
        let (temp, service) = setup_service();
        let first_item = service
            .create_item(CreateItemInput {
                title: "First project item".to_string(),
                description: "one".to_string(),
                priority: PriorityArg::Medium,
                parent: None,
            })
            .unwrap();
        let first_project = current_project(&service);

        let second_project = create_project(&service, &temp.path().join("second-project"));
        let second_item = seed_item_in_project(&service, &second_project, "Second project item");

        let current_overview = service.load_overview(None).unwrap();
        assert_eq!(
            current_overview.active_project.unwrap().public_id,
            first_project.public_id
        );
        assert_eq!(current_overview.items.len(), 1);
        assert_eq!(current_overview.items[0].public_id, first_item.public_id);

        let used_project = service.use_project(&second_project.public_id).unwrap();
        assert_eq!(used_project.public_id, second_project.public_id);

        let first_overview = service
            .load_overview(Some(&first_project.public_id))
            .unwrap();
        assert_eq!(
            first_overview.active_project.unwrap().public_id,
            first_project.public_id
        );
        assert_eq!(first_overview.items.len(), 1);
        assert_eq!(first_overview.items[0].public_id, first_item.public_id);

        let explicit_second = service
            .load_overview(Some(&second_project.public_id))
            .unwrap();
        assert_eq!(
            explicit_second.active_project.unwrap().public_id,
            second_project.public_id
        );
        assert_eq!(explicit_second.items.len(), 1);
        assert_eq!(explicit_second.items[0].public_id, second_item.public_id);
    }

    #[test]
    fn service_item_workflows_cover_detail_updates_and_undo() {
        let (_temp, service) = setup_service();

        let parent = service
            .create_item(CreateItemInput {
                title: "Parent".to_string(),
                description: "parent item".to_string(),
                priority: PriorityArg::High,
                parent: None,
            })
            .unwrap();
        let child = service
            .create_item(CreateItemInput {
                title: "Child".to_string(),
                description: "child item".to_string(),
                priority: PriorityArg::Low,
                parent: Some(parent.public_id.clone()),
            })
            .unwrap();
        let project = current_project(&service);

        let parent_detail = service
            .item_detail(&project.public_id, &parent.public_id)
            .unwrap();
        assert_eq!(parent_detail.children.len(), 1);
        assert_eq!(parent_detail.children[0].public_id, child.public_id);

        let updated_child = service
            .update_item(UpdateItemInput {
                item_id: child.public_id.clone(),
                title: "Updated child".to_string(),
                description: "updated description".to_string(),
                priority: PriorityArg::Urgent,
            })
            .unwrap();
        assert_eq!(updated_child.priority, "urgent");

        let ready_child = service.set_ready(&child.public_id, true).unwrap();
        assert!(ready_child.ready);
        let unready_child = service.set_ready(&child.public_id, false).unwrap();
        assert!(!unready_child.ready);

        let in_progress_child = service
            .set_status(&child.public_id, StatusArg::InProgress)
            .unwrap();
        assert_eq!(in_progress_child.status, "in_progress");
        let done_child = service
            .set_status(&child.public_id, StatusArg::Done)
            .unwrap();
        assert_eq!(done_child.status, "done");
        assert!(done_child.closed_at.is_some());

        let undone_command_id = latest_command_id(&service);
        let undo_result = service.undo(&undone_command_id).unwrap();
        assert_eq!(undo_result["reversed_command"], undone_command_id);
        let reopened_child = service
            .item_detail(&project.public_id, &child.public_id)
            .unwrap();
        assert_eq!(reopened_child.item.status, "in_progress");
        assert_eq!(reopened_child.item.closed_at, None);

        let moved_child = service.move_item(&child.public_id, None).unwrap();
        assert_eq!(moved_child.parent_id, None);
        let moved_detail = service
            .item_detail(&project.public_id, &child.public_id)
            .unwrap();
        assert_eq!(moved_detail.item.title, "Updated child");
        assert_eq!(moved_detail.item.description, "updated description");
        assert_eq!(moved_detail.item.priority, "urgent");

        let parent_after_move = service
            .item_detail(&project.public_id, &parent.public_id)
            .unwrap();
        assert!(parent_after_move.children.is_empty());

        service
            .set_block_relation(&child.public_id, &parent.public_id, true)
            .unwrap();
        let blocked_detail = service
            .item_detail(&project.public_id, &child.public_id)
            .unwrap();
        assert_eq!(blocked_detail.blockers, vec![parent.public_id.clone()]);

        service
            .set_block_relation(&child.public_id, &parent.public_id, false)
            .unwrap();
        let unblocked_detail = service
            .item_detail(&project.public_id, &child.public_id)
            .unwrap();
        assert!(unblocked_detail.blockers.is_empty());
    }

    #[test]
    fn validation_errors_are_returned_for_invalid_service_requests() {
        let (_temp, service) = setup_service();
        let item = service
            .create_item(CreateItemInput {
                title: "Task".to_string(),
                description: String::new(),
                priority: PriorityArg::Medium,
                parent: None,
            })
            .unwrap();

        let err = service
            .set_block_relation(&item.public_id, &item.public_id, true)
            .unwrap_err();
        match err {
            CliError::Validation { message, .. } => {
                assert!(message.contains("cannot block itself"));
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }
}

use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use crate::cli::{
    Commands, HistoryArgs, HistoryCommand, ItemArgs, ItemCommand, ItemCreateArgs, ItemListArgs,
    ItemMoveArgs, ItemReadyArgs, ItemStatusArgs, ItemUpdateArgs, NextArgs, ProjectArgs,
    ProjectCommand, ProjectUpdateArgs, ReviewArgs, ReviewCommand, UndoArgs,
};
use crate::db::{
    initialize_database, now_string, open_connection, owner_id, resolve_db_path, with_write,
};
use crate::domain::{
    ItemListFilter, StatusArg, TreeNode, WorkItemRecord, blocker_relation_event_key, bool_to_i64,
    join_ids, join_item_ids, work_item_event_key, work_item_state,
};
use crate::error::{CliError, CliResult};
use crate::git::require_repo_root;
use crate::output::{emit_project, emit_value, render_tree};
use crate::repo::{
    allocate_project_item_number, build_tree, create_command, ensure_no_block_cycle,
    ensure_valid_parent, get_command_history, get_item_by_public_id,
    get_item_by_public_id_readonly, get_or_create_project, insert_event, list_blocked_by,
    list_blockers, list_children, list_commands, list_item_history, list_items, list_projects,
    list_root_items, resolve_active_item, resolve_active_project, resolve_active_project_readonly,
    resolve_active_project_resolution, resolve_active_project_with_override, resolve_parent_row_id,
    resolve_project_tx_with_override, select_next_items, set_project_override, undo_command,
    update_project_prefix,
};

pub struct App {
    db_path: std::path::PathBuf,
    json_output: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewTreeNode {
    item: WorkItemRecord,
    review_state: &'static str,
    review_reason: String,
    needs_review: bool,
    has_pending_descendants: bool,
    has_unready_descendants: bool,
    pending_descendants: usize,
    unready_descendants: usize,
    children: Vec<ReviewTreeNode>,
}

#[derive(Debug, Clone, Serialize)]
struct NextItemExplanation {
    item_id: String,
    reason: String,
}

impl App {
    pub fn new(json_output: bool) -> CliResult<Self> {
        let db_path = resolve_db_path()?;
        initialize_database(&db_path)?;
        Ok(Self {
            db_path,
            json_output,
        })
    }

    pub fn dispatch(&self, command: Commands) -> CliResult<()> {
        match command {
            Commands::Init => self.init_project(),
            Commands::Project(ProjectArgs { command }) => self.project_command(command),
            Commands::Item(ItemArgs { command }) => self.item_command(command),
            Commands::Review(ReviewArgs { command }) => self.review_command(command),
            Commands::Next(args) => self.next(args),
            Commands::History(HistoryArgs { command }) => self.history_command(command),
            Commands::Undo(args) => self.undo(args),
            Commands::Ui => unreachable!("ui is handled before CLI dispatch"),
        }
    }

    fn init_project(&self) -> CliResult<()> {
        let repo_root = require_repo_root(self.json_output)?;
        let mut conn = open_connection(&self.db_path)?;
        let owner = owner_id();
        let project = with_write(&mut conn, &owner, |tx| {
            get_or_create_project(tx, &repo_root, true)
        })?;
        emit_value(
            self.json_output,
            &json!({ "project": project, "created": true }),
        )
    }

    fn project_command(&self, command: ProjectCommand) -> CliResult<()> {
        match command {
            ProjectCommand::Show(args) => {
                let mut conn = open_connection(&self.db_path)?;
                if args.explain {
                    let resolution =
                        resolve_active_project_resolution(&conn, true, self.json_output)?;
                    crate::output::emit_project_resolution(
                        self.json_output,
                        "Active project",
                        &resolution,
                    )
                } else {
                    let project = resolve_active_project(&mut conn, true, self.json_output)?;
                    emit_project(self.json_output, "Active project", &project)
                }
            }
            ProjectCommand::List => {
                let conn = open_connection(&self.db_path)?;
                let projects = list_projects(&conn)?;
                if self.json_output {
                    emit_value(true, &json!({ "projects": projects }))
                } else {
                    if projects.is_empty() {
                        println!("No projects found.");
                    } else {
                        for project in projects {
                            println!(
                                "{}\t{}\t{}",
                                project.public_id,
                                project.name,
                                project.repo_root.unwrap_or_else(|| "-".to_string())
                            );
                        }
                    }
                    Ok(())
                }
            }
            ProjectCommand::Use { project_id } => {
                let mut conn = open_connection(&self.db_path)?;
                let project = with_write(&mut conn, &owner_id(), |tx| {
                    set_project_override(tx, &project_id)
                })?;
                emit_project(self.json_output, "Selected project", &project)
            }
            ProjectCommand::Update(ProjectUpdateArgs { project_id, prefix }) => {
                let mut conn = open_connection(&self.db_path)?;
                let owner = owner_id();
                let project = with_write(&mut conn, &owner, |tx| {
                    update_project_prefix(tx, &project_id, &prefix)
                })?;
                emit_project(self.json_output, "Updated project", &project)
            }
        }
    }

    fn item_command(&self, command: ItemCommand) -> CliResult<()> {
        match command {
            ItemCommand::Create(args) => self.item_create(args),
            ItemCommand::List(args) => self.item_list(args),
            ItemCommand::Show { item_id } => self.item_show(&item_id),
            ItemCommand::Update(args) => self.item_update(args),
            ItemCommand::Status(args) => self.item_status(args),
            ItemCommand::Ready(args) => self.item_ready_state(args, true),
            ItemCommand::Unready(args) => self.item_ready_state(args, false),
            ItemCommand::Block(args) => self.item_block(
                &args.item_id,
                &args.blocker_id,
                args.project.as_deref(),
                true,
            ),
            ItemCommand::Unblock(args) => self.item_block(
                &args.item_id,
                &args.blocker_id,
                args.project.as_deref(),
                false,
            ),
            ItemCommand::Blockers { item_id } => self.item_blockers(&item_id),
            ItemCommand::Move(args) => self.item_move(args),
            ItemCommand::Children { item_id } => self.item_children(&item_id),
            ItemCommand::Tree { item_id } => self.item_tree(item_id.as_deref()),
        }
    }

    fn review_command(&self, command: ReviewCommand) -> CliResult<()> {
        match command {
            ReviewCommand::Tree { item_id } => self.review_tree(item_id.as_deref()),
        }
    }

    fn item_create(&self, args: ItemCreateArgs) -> CliResult<()> {
        let mut conn = open_connection(&self.db_path)?;
        let owner = owner_id();
        let (project, item) = with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx_with_override(
                tx,
                args.project.as_deref(),
                true,
                self.json_output,
            )?;
            let parent = match args.parent.as_deref() {
                Some(parent_id) => Some(get_item_by_public_id(tx, project.id, parent_id)?),
                None => None,
            };

            let number = allocate_project_item_number(tx, project.id)?;
            let public_id = format!("{}-{number}", project.record.item_prefix);
            let now = now_string();
            let parent_public_id = parent.as_ref().map(|item| item.record.public_id.clone());
            let parent_row_id = parent.as_ref().map(|item| item.row_id);
            let item = WorkItemRecord {
                public_id: public_id.clone(),
                project_id: project.id,
                title: args.title.clone(),
                description: args.description.clone(),
                ready: false,
                status: StatusArg::Todo.to_string(),
                priority: args.priority.to_string(),
                parent_id: parent_public_id,
                created_at: now.clone(),
                updated_at: now,
                closed_at: None,
                version: 1,
            };

            let command = create_command(tx, Some(project.id), "item.create", None)?;
            tx.execute(
                "INSERT INTO work_items (public_id, project_id, title, description, ready, status, priority, parent_id, created_at, updated_at, closed_at, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![item.public_id, item.project_id, item.title, item.description, bool_to_i64(item.ready), item.status, item.priority, parent_row_id, item.created_at, item.updated_at, item.closed_at, item.version],
            )?;
            let persisted = get_item_by_public_id(tx, project.id, &public_id)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &public_id),
                "create",
                None,
                Some(work_item_state(&persisted.record)),
            )?;
            Ok((project.record, persisted.record))
        })?;

        crate::output::emit_item_for_project(self.json_output, "Created item", &project, &item)
    }

    fn item_list(&self, args: ItemListArgs) -> CliResult<()> {
        let mut conn = open_connection(&self.db_path)?;
        let project = resolve_active_project_with_override(
            &mut conn,
            args.project.as_deref(),
            self.json_output,
        )?;
        let filters = ItemListFilter::from(&args);
        let items = list_items(&conn, project.id, &filters)?;
        if self.json_output {
            emit_value(true, &json!({ "items": items }))
        } else {
            if items.is_empty() {
                println!("No items found.");
            } else {
                for item in items {
                    println!(
                        "{}\t{}\t{}\tready={}\t{}",
                        item.public_id, item.status, item.priority, item.ready, item.title
                    );
                }
            }
            Ok(())
        }
    }

    fn item_show(&self, item_id: &str) -> CliResult<()> {
        let conn = open_connection(&self.db_path)?;
        let (project, item) = resolve_active_item(&conn, item_id, self.json_output)?;
        let children = list_children(&conn, project.id, item.row_id)?;
        let blockers = list_blockers(&conn, project.id, &item.record.public_id)?;
        let blocked_by = list_blocked_by(&conn, project.id, &item.record.public_id)?;

        if self.json_output {
            emit_value(
                true,
                &json!({ "item": item.record, "children": children, "blockers": blockers, "blocked_by": blocked_by }),
            )
        } else {
            println!("{}: {}", item.record.public_id, item.record.title);
            println!(
                "status={} priority={} ready={}",
                item.record.status, item.record.priority, item.record.ready
            );
            if !item.record.description.is_empty() {
                println!("{}", item.record.description);
            }
            println!(
                "parent={}",
                item.record
                    .parent_id
                    .clone()
                    .unwrap_or_else(|| "-".to_string())
            );
            println!("children={}", join_item_ids(&children));
            println!("blocked_by={}", join_ids(&blocked_by));
            println!("blocks={}", join_ids(&blockers));
            Ok(())
        }
    }

    fn item_update(&self, args: ItemUpdateArgs) -> CliResult<()> {
        if args.parent.is_some() && args.root {
            return Err(CliError::Usage {
                message: "cannot use --parent and --root together".to_string(),
                json: self.json_output,
            });
        }

        let mut conn = open_connection(&self.db_path)?;
        let owner = owner_id();
        let (project, updated) = with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx_with_override(
                tx,
                args.project.as_deref(),
                true,
                self.json_output,
            )?;
            let existing = get_item_by_public_id(tx, project.id, &args.item_id)?;
            let before = work_item_state(&existing.record);
            let mut item = existing.record.clone();

            if let Some(title) = args.title.clone() {
                item.title = title;
            }
            if let Some(description) = args.description.clone() {
                item.description = description;
            }
            if let Some(status) = args.status {
                item.status = status.to_string();
                item.closed_at = if status.is_terminal() {
                    Some(now_string())
                } else {
                    None
                };
            }
            if let Some(priority) = args.priority {
                item.priority = priority.to_string();
            }
            if args.root {
                item.parent_id = None;
            }
            if let Some(parent_id) = args.parent.as_deref() {
                let parent = get_item_by_public_id(tx, project.id, parent_id)?;
                ensure_valid_parent(tx, project.id, existing.row_id, Some(parent.row_id))?;
                item.parent_id = Some(parent.record.public_id);
            }

            item.updated_at = now_string();
            item.version += 1;

            tx.execute(
                "UPDATE work_items SET title=?1, description=?2, status=?3, priority=?4, parent_id=?5, updated_at=?6, closed_at=?7, version=?8 WHERE id=?9",
                rusqlite::params![item.title, item.description, item.status, item.priority, resolve_parent_row_id(tx, project.id, item.parent_id.as_deref())?, item.updated_at, item.closed_at, item.version, existing.row_id],
            )?;

            let command = create_command(tx, Some(project.id), "item.update", None)?;
            let persisted = get_item_by_public_id(tx, project.id, &args.item_id)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &item.public_id),
                "update",
                Some(before),
                Some(work_item_state(&persisted.record)),
            )?;
            Ok((project.record, persisted.record))
        })?;

        crate::output::emit_item_for_project(self.json_output, "Updated item", &project, &updated)
    }

    fn item_status(&self, args: ItemStatusArgs) -> CliResult<()> {
        self.item_update(ItemUpdateArgs {
            item_id: args.item_id,
            title: None,
            description: None,
            status: Some(args.status),
            priority: None,
            parent: None,
            root: false,
            project: args.project,
        })
    }

    fn item_ready_state(&self, args: ItemReadyArgs, ready: bool) -> CliResult<()> {
        let mut conn = open_connection(&self.db_path)?;
        let owner = owner_id();
        let (project, updated) = with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx_with_override(
                tx,
                args.project.as_deref(),
                true,
                self.json_output,
            )?;
            let existing = get_item_by_public_id(tx, project.id, &args.item_id)?;
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
            )?;

            let command = create_command(
                tx,
                Some(project.id),
                if ready { "item.ready" } else { "item.unready" },
                None,
            )?;
            let persisted = get_item_by_public_id(tx, project.id, &args.item_id)?;
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "work_item",
                &work_item_event_key(project.id, &item.public_id),
                if ready { "ready" } else { "unready" },
                Some(before),
                Some(work_item_state(&persisted.record)),
            )?;
            Ok((project.record, persisted.record))
        })?;

        crate::output::emit_item_for_project(
            self.json_output,
            if ready {
                "Marked ready"
            } else {
                "Marked unready"
            },
            &project,
            &updated,
        )
    }

    fn item_block(
        &self,
        item_id: &str,
        blocker_id: &str,
        project_id: Option<&str>,
        add: bool,
    ) -> CliResult<()> {
        let mut conn = open_connection(&self.db_path)?;
        let owner = owner_id();
        let payload = with_write(&mut conn, &owner, |tx| {
            let project = resolve_project_tx_with_override(tx, project_id, true, self.json_output)?;
            let blocked = get_item_by_public_id(tx, project.id, item_id)?;
            let blocker = get_item_by_public_id(tx, project.id, blocker_id)?;

            if blocked.row_id == blocker.row_id {
                return crate::error::validation("an item cannot block itself");
            }

            if add {
                ensure_no_block_cycle(tx, blocker.row_id, blocked.row_id)?;
                tx.execute(
                    "INSERT OR IGNORE INTO work_item_blockers (blocker_id, blocked_id, created_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![blocker.row_id, blocked.row_id, now_string()],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM work_item_blockers WHERE blocker_id=?1 AND blocked_id=?2",
                    rusqlite::params![blocker.row_id, blocked.row_id],
                )?;
            }

            let command = create_command(
                tx,
                Some(project.id),
                if add { "item.block" } else { "item.unblock" },
                None,
            )?;
            let relation_key = blocker_relation_event_key(
                project.id,
                &blocker.record.public_id,
                &blocked.record.public_id,
            );
            insert_event(
                tx,
                command.id,
                Some(project.id),
                "blocker_relation",
                &relation_key,
                if add { "create" } else { "delete" },
                if add {
                    None
                } else {
                    Some(
                        json!({"blocker_id": blocker.record.public_id, "blocked_id": blocked.record.public_id}),
                    )
                },
                if add {
                    Some(
                        json!({"blocker_id": blocker.record.public_id, "blocked_id": blocked.record.public_id}),
                    )
                } else {
                    None
                },
            )?;
            Ok(json!({ "blocked": blocked.record, "blocker": blocker.record, "added": add }))
        })?;

        emit_value(self.json_output, &payload)
    }

    fn item_blockers(&self, item_id: &str) -> CliResult<()> {
        let conn = open_connection(&self.db_path)?;
        let (project, item) = resolve_active_item(&conn, item_id, self.json_output)?;
        let blockers = list_blockers(&conn, project.id, &item.record.public_id)?;
        let blocked_by = list_blocked_by(&conn, project.id, &item.record.public_id)?;
        if self.json_output {
            emit_value(
                true,
                &json!({ "item": item.record, "blockers": blockers, "blocked_by": blocked_by }),
            )
        } else {
            println!("{}", item.record.public_id);
            println!("blocks={}", join_ids(&blockers));
            println!("blocked_by={}", join_ids(&blocked_by));
            Ok(())
        }
    }

    fn item_move(&self, args: ItemMoveArgs) -> CliResult<()> {
        self.item_update(ItemUpdateArgs {
            item_id: args.item_id,
            title: None,
            description: None,
            status: None,
            priority: None,
            parent: args.parent,
            root: args.root,
            project: args.project,
        })
    }

    fn item_children(&self, item_id: &str) -> CliResult<()> {
        let conn = open_connection(&self.db_path)?;
        let (project, item) = resolve_active_item(&conn, item_id, self.json_output)?;
        let children = list_children(&conn, project.id, item.row_id)?;
        if self.json_output {
            emit_value(true, &json!({ "item": item.record, "children": children }))
        } else {
            if children.is_empty() {
                println!("No children.");
            } else {
                for child in children {
                    println!("{}\t{}", child.public_id, child.title);
                }
            }
            Ok(())
        }
    }

    fn item_tree(&self, item_id: Option<&str>) -> CliResult<()> {
        let conn = open_connection(&self.db_path)?;
        let project = resolve_active_project_readonly(&conn, true, self.json_output)?;
        let roots = if let Some(item_id) = item_id {
            vec![get_item_by_public_id_readonly(&conn, project.id, item_id)?.record]
        } else {
            list_root_items(&conn, project.id)?
        };
        let tree = roots
            .into_iter()
            .map(|item| build_tree(&conn, project.id, item))
            .collect::<anyhow::Result<Vec<TreeNode>>>()?;

        if self.json_output {
            emit_value(true, &json!({ "tree": tree }))
        } else {
            for node in &tree {
                render_tree(node, 0);
            }
            Ok(())
        }
    }

    fn review_tree(&self, item_id: Option<&str>) -> CliResult<()> {
        let conn = open_connection(&self.db_path)?;
        let project = resolve_active_project_readonly(&conn, true, self.json_output)?;
        let roots = if let Some(item_id) = item_id {
            vec![get_item_by_public_id_readonly(&conn, project.id, item_id)?.record]
        } else {
            list_root_items(&conn, project.id)?
        };
        let mut tree = roots
            .into_iter()
            .map(|item| build_tree(&conn, project.id, item))
            .collect::<anyhow::Result<Vec<TreeNode>>>()?
            .into_iter()
            .map(build_review_tree)
            .collect::<Vec<_>>();
        tree.sort_by(|left, right| review_sort_key(right).cmp(&review_sort_key(left)));

        if self.json_output {
            emit_value(true, &json!({ "tree": tree }))
        } else {
            for node in &tree {
                render_review_tree(node, 0);
            }
            Ok(())
        }
    }

    fn next(&self, args: NextArgs) -> CliResult<()> {
        let conn = open_connection(&self.db_path)?;
        let project = resolve_active_project_readonly(&conn, true, self.json_output)?;
        let items = loop {
            let conn = open_connection(&self.db_path)?;
            let items = select_next_items(&conn, project.id, args.limit)?;
            if !items.is_empty() {
                break items;
            }
            if !args.wait {
                return Err(CliError::EmptyResult {
                    message: "No unblocked work items are available.".to_string(),
                    json: self.json_output,
                });
            }
            thread::sleep(Duration::from_millis(250));
        };
        let explanations = items
            .iter()
            .map(|item| {
                Ok(NextItemExplanation {
                    item_id: item.public_id.clone(),
                    reason: next_reason(&conn, project.id, item)?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        if self.json_output {
            emit_value(
                true,
                &json!({ "items": items, "explanations": explanations }),
            )
        } else {
            for (item, explanation) in items.iter().zip(explanations.iter()) {
                println!(
                    "{}\t{}\t{}\t{}",
                    item.public_id, item.priority, item.title, explanation.reason
                );
            }
            Ok(())
        }
    }

    fn history_command(&self, command: HistoryCommand) -> CliResult<()> {
        match command {
            HistoryCommand::Show { item_id } => {
                let conn = open_connection(&self.db_path)?;
                let (project, item) = resolve_active_item(&conn, &item_id, self.json_output)?;
                let history = list_item_history(&conn, project.id, &item.record.public_id)?;
                emit_value(
                    self.json_output,
                    &json!({ "item": item.record, "events": history }),
                )
            }
            HistoryCommand::Command { command_id } => {
                let conn = open_connection(&self.db_path)?;
                let history = get_command_history(&conn, &command_id)?;
                emit_value(
                    self.json_output,
                    &json!({ "command": history.0, "events": history.1 }),
                )
            }
            HistoryCommand::List => {
                let conn = open_connection(&self.db_path)?;
                let project = resolve_active_project_readonly(&conn, true, self.json_output)?;
                let commands = list_commands(&conn, project.id)?;
                emit_value(self.json_output, &json!({ "commands": commands }))
            }
        }
    }

    fn undo(&self, args: UndoArgs) -> CliResult<()> {
        let mut conn = open_connection(&self.db_path)?;
        let owner = owner_id();
        let output = with_write(&mut conn, &owner, |tx| undo_command(tx, &args.command_id))?;
        emit_value(self.json_output, &output)
    }
}

fn build_review_tree(node: TreeNode) -> ReviewTreeNode {
    let mut children = node
        .children
        .into_iter()
        .map(build_review_tree)
        .collect::<Vec<_>>();
    children.sort_by(|left, right| review_sort_key(right).cmp(&review_sort_key(left)));

    let pending_descendants = children
        .iter()
        .map(|child| {
            usize::from(
                child.item.status != StatusArg::Done.to_string()
                    && child.item.status != StatusArg::Cancelled.to_string(),
            ) + child.pending_descendants
        })
        .sum();
    let unready_descendants = children
        .iter()
        .map(|child| usize::from(!child.item.ready) + child.unready_descendants)
        .sum();
    let has_pending_descendants = pending_descendants > 0;
    let has_unready_descendants = unready_descendants > 0;
    let needs_review = !node.item.ready;
    let review_state = if needs_review {
        "REVIEW"
    } else if has_unready_descendants {
        "WAIT"
    } else if has_pending_descendants {
        "OPEN"
    } else {
        "CLEAR"
    };
    let review_reason = if needs_review {
        "item is not ready".to_string()
    } else if has_unready_descendants {
        format!("waiting on {unready_descendants} unready descendant(s)")
    } else if has_pending_descendants {
        format!("waiting on {pending_descendants} open descendant(s)")
    } else {
        "ready and descendants are closed or ready".to_string()
    };

    ReviewTreeNode {
        item: node.item,
        review_state,
        review_reason,
        needs_review,
        has_pending_descendants,
        has_unready_descendants,
        pending_descendants,
        unready_descendants,
        children,
    }
}

fn review_sort_key(node: &ReviewTreeNode) -> (bool, bool, bool, &str) {
    (
        node.needs_review,
        node.has_unready_descendants,
        node.has_pending_descendants,
        node.item.public_id.as_str(),
    )
}

fn render_review_tree(node: &ReviewTreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    println!(
        "{}{} {} [{} ready={}] pending={} unready={} {}",
        indent,
        node.review_state,
        node.item.public_id,
        node.item.status,
        node.item.ready,
        node.pending_descendants,
        node.unready_descendants,
        node.item.title,
    );
    println!("{}  reason={}", indent, node.review_reason);
    for child in &node.children {
        render_review_tree(child, depth + 1);
    }
}

fn next_reason(
    conn: &rusqlite::Connection,
    project_id: i64,
    item: &WorkItemRecord,
) -> anyhow::Result<String> {
    let blockers = crate::repo::list_blockers(conn, project_id, &item.public_id)?;
    let row = crate::repo::get_item_by_public_id_readonly(conn, project_id, &item.public_id)?;
    let open_children = crate::repo::list_children(conn, project_id, row.row_id)?
        .into_iter()
        .filter(|child| {
            child.status != StatusArg::Done.to_string()
                && child.status != StatusArg::Cancelled.to_string()
        })
        .count();
    let activity = if item.status == StatusArg::InProgress.to_string() {
        "already in progress"
    } else {
        "ready to start"
    };
    Ok(format!(
        "{activity}; blockers={} open_children={open_children}",
        blockers.len()
    ))
}

mod input;

use anyhow::Result;
use gpui::{
    App, Application, AsyncApp, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding,
    SharedString, Subscription, WeakEntity, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, rgb, size,
};
use std::collections::HashSet;
use std::time::Duration;

use crate::domain::{CommandRecord, EventRecord, PriorityArg, StatusArg, TreeNode};
use crate::services::{
    CreateItemInput, IssueService, ItemDetail, OverviewSnapshot, UpdateItemInput,
};
use input::TextInput;

pub fn run_ui() -> Result<()> {
    let service = IssueService::new()?;
    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", input::Backspace, None),
            KeyBinding::new("delete", input::Delete, None),
            KeyBinding::new("left", input::Left, None),
            KeyBinding::new("right", input::Right, None),
            KeyBinding::new("shift-left", input::SelectLeft, None),
            KeyBinding::new("shift-right", input::SelectRight, None),
            KeyBinding::new("cmd-a", input::SelectAll, None),
            KeyBinding::new("cmd-v", input::Paste, None),
            KeyBinding::new("cmd-c", input::Copy, None),
            KeyBinding::new("cmd-x", input::Cut, None),
            KeyBinding::new("home", input::Home, None),
            KeyBinding::new("end", input::End, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1500.0), px(940.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let ui = cx.new(|cx| IssueUi::new(service.clone(), cx));
                ui.update(cx, |this, cx| this.install_watchers(window, cx));
                ui
            },
        )
        .expect("window");
        cx.activate(true);
    });
    Ok(())
}

#[derive(Clone)]
struct FlashMessage {
    text: SharedString,
    is_error: bool,
}

struct IssueUi {
    service: IssueService,
    focus_handle: FocusHandle,
    selected_project_id: Option<String>,
    selected_item_id: Option<String>,
    selected_command_id: Option<String>,
    overview: Option<OverviewSnapshot>,
    item_detail: Option<ItemDetail>,
    command_detail: Option<(CommandRecord, Vec<EventRecord>)>,
    flash: Option<FlashMessage>,
    subscriptions: Vec<Subscription>,
    create_priority: PriorityArg,
    edit_priority: PriorityArg,
    create_title: Entity<TextInput>,
    create_description: Entity<TextInput>,
    create_parent: Entity<TextInput>,
    edit_title: Entity<TextInput>,
    edit_description: Entity<TextInput>,
    move_parent: Entity<TextInput>,
    block_target: Entity<TextInput>,
    block_by: Entity<TextInput>,
}

impl IssueUi {
    const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

    fn new(service: IssueService, cx: &mut Context<Self>) -> Self {
        let create_title = cx.new(|cx| TextInput::new(cx, "New item title"));
        let create_description = cx.new(|cx| TextInput::new(cx, "Description"));
        let create_parent = cx.new(|cx| TextInput::new(cx, "Parent WI id (optional)"));
        let edit_title = cx.new(|cx| TextInput::new(cx, "Selected item title"));
        let edit_description = cx.new(|cx| TextInput::new(cx, "Selected item description"));
        let move_parent = cx.new(|cx| TextInput::new(cx, "New parent WI id, blank for root"));
        let block_target = cx.new(|cx| TextInput::new(cx, "Blocked item id"));
        let block_by = cx.new(|cx| TextInput::new(cx, "Blocker item id"));
        let mut this = Self {
            service,
            focus_handle: cx.focus_handle(),
            selected_project_id: None,
            selected_item_id: None,
            selected_command_id: None,
            overview: None,
            item_detail: None,
            command_detail: None,
            flash: None,
            subscriptions: Vec::new(),
            create_priority: PriorityArg::Medium,
            edit_priority: PriorityArg::Medium,
            create_title,
            create_description,
            create_parent,
            edit_title,
            edit_description,
            move_parent,
            block_target,
            block_by,
        };
        this.refresh(cx);
        this
    }

    fn install_watchers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.refresh_preserving_inputs(cx);
                }
            }));

        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut async_cx = cx.clone();
            async move {
                loop {
                    async_cx
                        .background_executor()
                        .timer(Self::AUTO_REFRESH_INTERVAL)
                        .await;
                    if this
                        .update(&mut async_cx, |this, cx| this.refresh_preserving_inputs(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_internal(cx, false);
    }

    fn refresh_preserving_inputs(&mut self, cx: &mut Context<Self>) {
        self.refresh_internal(cx, true);
    }

    fn refresh_internal(&mut self, cx: &mut Context<Self>, preserve_selected_inputs: bool) {
        match self
            .service
            .load_overview(self.selected_project_id.as_deref())
        {
            Ok(overview) => {
                if self.selected_project_id.is_none() {
                    self.selected_project_id = overview
                        .active_project
                        .as_ref()
                        .map(|p| p.public_id.clone());
                }
                let active_project_id = overview
                    .active_project
                    .as_ref()
                    .map(|p| p.public_id.clone());
                self.overview = Some(overview);
                if let (Some(project_id), Some(item_id)) = (
                    active_project_id.as_deref(),
                    self.selected_item_id.as_deref(),
                ) {
                    match self.service.item_detail(project_id, item_id) {
                        Ok(detail) => {
                            if !(preserve_selected_inputs && self.has_unsaved_selected_edits(cx)) {
                                self.sync_edit_inputs(&detail, cx);
                                self.sync_selected_priority(&detail, cx);
                            }
                            self.item_detail = Some(detail);
                        }
                        Err(_) => {
                            self.selected_item_id = None;
                            self.item_detail = None;
                        }
                    }
                } else {
                    self.item_detail = None;
                }
                if let Some(command_id) = self.selected_command_id.clone() {
                    self.command_detail = self.service.command_history(&command_id).ok();
                }
            }
            Err(err) => {
                self.flash = Some(FlashMessage {
                    text: err.to_string().into(),
                    is_error: true,
                });
                self.overview = None;
                self.item_detail = None;
            }
        }
        cx.notify();
    }

    fn has_unsaved_selected_edits(&self, cx: &App) -> bool {
        let Some(detail) = &self.item_detail else {
            return false;
        };

        Self::read_input(&self.edit_title, cx) != detail.item.title
            || Self::read_input(&self.edit_description, cx) != detail.item.description
            || Self::read_input(&self.move_parent, cx)
                != detail.item.parent_id.clone().unwrap_or_default()
            || Self::read_input(&self.block_target, cx) != detail.item.public_id
            || self.edit_priority.to_string() != detail.item.priority
    }

    fn sync_edit_inputs(&self, detail: &ItemDetail, cx: &mut Context<Self>) {
        let title = detail.item.title.clone();
        let description = detail.item.description.clone();
        self.edit_title
            .update(cx, |input, cx| input.set_value(title, cx));
        self.edit_description
            .update(cx, |input, cx| input.set_value(description, cx));
        self.move_parent.update(cx, |input, cx| {
            input.set_value(detail.item.parent_id.clone().unwrap_or_default(), cx)
        });
        self.block_target.update(cx, |input, cx| {
            input.set_value(detail.item.public_id.clone(), cx)
        });
    }

    fn sync_selected_priority(&mut self, detail: &ItemDetail, cx: &mut Context<Self>) {
        self.edit_priority = detail.item.priority.parse().unwrap_or(PriorityArg::Medium);
        cx.notify();
    }

    fn set_flash(&mut self, text: impl Into<SharedString>, is_error: bool, cx: &mut Context<Self>) {
        self.flash = Some(FlashMessage {
            text: text.into(),
            is_error,
        });
        cx.notify();
    }

    fn selected_project_id(&self) -> Option<&str> {
        self.overview
            .as_ref()
            .and_then(|o| o.active_project.as_ref())
            .map(|p| p.public_id.as_str())
            .or(self.selected_project_id.as_deref())
    }

    fn read_input(entity: &Entity<TextInput>, cx: &App) -> String {
        entity.read(cx).value()
    }

    fn set_input_value(
        entity: &Entity<TextInput>,
        value: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        entity.update(cx, |input, cx| input.set_value(value, cx));
    }

    fn set_create_priority_click(&mut self, priority: PriorityArg, cx: &mut Context<Self>) {
        self.create_priority = priority;
        cx.notify();
    }

    fn set_edit_priority_click(&mut self, priority: PriorityArg, cx: &mut Context<Self>) {
        self.edit_priority = priority;
        cx.notify();
    }

    fn choose_create_parent_candidate(&mut self, item_id: String, cx: &mut Context<Self>) {
        Self::set_input_value(&self.create_parent, item_id, cx);
    }

    fn choose_move_parent_candidate(&mut self, item_id: String, cx: &mut Context<Self>) {
        Self::set_input_value(&self.move_parent, item_id, cx);
    }

    fn choose_root_parent(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.move_parent.update(cx, |input, cx| input.clear(cx));
    }

    fn choose_blocker_candidate(&mut self, item_id: String, cx: &mut Context<Self>) {
        Self::set_input_value(&self.block_by, item_id, cx);
    }

    fn choose_selected_as_block_target(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item_id) = self.selected_item_id.clone() {
            Self::set_input_value(&self.block_target, item_id, cx);
        }
    }

    fn create_parent_candidates(&self) -> Vec<crate::domain::WorkItemRecord> {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        if let Some(overview) = &self.overview {
            for node in &overview.tree {
                push_candidate(&mut candidates, &mut seen, &node.item);
            }
            for item in &overview.next_items {
                push_candidate(&mut candidates, &mut seen, item);
            }
            for item in &overview.items {
                push_candidate(&mut candidates, &mut seen, item);
            }
        }
        candidates.truncate(8);
        candidates
    }

    fn move_parent_candidates(&self) -> Vec<crate::domain::WorkItemRecord> {
        let Some(selected_id) = self.selected_item_id.as_deref() else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut blocked = HashSet::new();
        blocked.insert(selected_id.to_string());
        if let Some(overview) = &self.overview {
            collect_descendant_ids(&overview.tree, selected_id, &mut blocked);
        }

        let mut candidates = Vec::new();
        if let (Some(overview), Some(detail)) = (&self.overview, &self.item_detail) {
            if let Some(parent_id) = detail.item.parent_id.as_deref()
                && let Some(item) = overview
                    .items
                    .iter()
                    .find(|item| item.public_id == parent_id)
            {
                push_candidate_if_allowed(&mut candidates, &mut seen, &blocked, item);
            }
            for node in &overview.tree {
                push_candidate_if_allowed(&mut candidates, &mut seen, &blocked, &node.item);
            }
            for item in &overview.next_items {
                push_candidate_if_allowed(&mut candidates, &mut seen, &blocked, item);
            }
            for item in &overview.items {
                push_candidate_if_allowed(&mut candidates, &mut seen, &blocked, item);
            }
        }
        candidates.truncate(8);
        candidates
    }

    fn blocker_candidates(&self) -> Vec<crate::domain::WorkItemRecord> {
        let Some(selected_id) = self.selected_item_id.as_deref() else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        if let (Some(overview), Some(detail)) = (&self.overview, &self.item_detail) {
            for blocker_id in &detail.blocked_by {
                if let Some(item) = overview
                    .items
                    .iter()
                    .find(|item| item.public_id == *blocker_id)
                {
                    push_candidate(&mut candidates, &mut seen, item);
                }
            }
            for item in &overview.next_items {
                if item.public_id != selected_id {
                    push_candidate(&mut candidates, &mut seen, item);
                }
            }
            for item in &overview.items {
                if item.public_id != selected_id {
                    push_candidate(&mut candidates, &mut seen, item);
                }
            }
        }
        candidates.truncate(8);
        candidates
    }

    fn with_result<T>(
        &mut self,
        result: crate::error::CliResult<T>,
        success: &str,
        cx: &mut Context<Self>,
    ) -> Option<T> {
        match result {
            Ok(value) => {
                self.set_flash(success.to_string(), false, cx);
                Some(value)
            }
            Err(err) => {
                self.set_flash(err.to_string(), true, cx);
                None
            }
        }
    }

    fn refresh_click(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    fn init_click(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self
            .with_result(
                self.service.init_current_repo_project(),
                "Initialized current repo project.",
                cx,
            )
            .is_some()
        {
            self.refresh(cx);
        }
    }

    fn create_click(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let title = Self::read_input(&self.create_title, cx);
        let description = Self::read_input(&self.create_description, cx);
        let parent = Self::read_input(&self.create_parent, cx);
        if title.trim().is_empty() {
            self.set_flash("New item title is required.", true, cx);
            return;
        }
        let created = self.with_result(
            self.service.create_item(CreateItemInput {
                title: title.trim().to_string(),
                description: description.trim().to_string(),
                priority: self.create_priority,
                parent: if parent.trim().is_empty() {
                    None
                } else {
                    Some(parent.trim().to_string())
                },
            }),
            "Created item.",
            cx,
        );
        if let Some(item) = created {
            self.selected_item_id = Some(item.public_id);
            self.create_title.update(cx, |input, cx| input.clear(cx));
            self.create_description
                .update(cx, |input, cx| input.clear(cx));
            self.create_parent.update(cx, |input, cx| input.clear(cx));
            self.create_priority = PriorityArg::Medium;
            self.refresh(cx);
        }
    }

    fn save_item_click(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item_id) = self.selected_item_id.clone() else {
            self.set_flash("Select an item first.", true, cx);
            return;
        };
        let title = Self::read_input(&self.edit_title, cx);
        if title.trim().is_empty() {
            self.set_flash("Selected item title is required.", true, cx);
            return;
        }
        let description = Self::read_input(&self.edit_description, cx);
        if self
            .with_result(
                self.service.update_item(UpdateItemInput {
                    item_id,
                    title: title.trim().to_string(),
                    description: description.trim().to_string(),
                    priority: self.edit_priority,
                }),
                "Updated item.",
                cx,
            )
            .is_some()
        {
            self.refresh(cx);
        }
    }

    fn move_item_click(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item_id) = self.selected_item_id.clone() else {
            self.set_flash("Select an item first.", true, cx);
            return;
        };
        let parent = Self::read_input(&self.move_parent, cx);
        if self
            .with_result(
                self.service.move_item(&item_id, Some(parent.trim())),
                "Moved item.",
                cx,
            )
            .is_some()
        {
            self.refresh(cx);
        }
    }

    fn set_status_click(&mut self, status: StatusArg, cx: &mut Context<Self>) {
        let Some(item_id) = self.selected_item_id.clone() else {
            self.set_flash("Select an item first.", true, cx);
            return;
        };
        if self
            .with_result(
                self.service.set_status(&item_id, status),
                "Updated status.",
                cx,
            )
            .is_some()
        {
            self.refresh(cx);
        }
    }

    fn set_ready_click(&mut self, ready: bool, cx: &mut Context<Self>) {
        let Some(item_id) = self.selected_item_id.clone() else {
            self.set_flash("Select an item first.", true, cx);
            return;
        };
        if self
            .with_result(
                self.service.set_ready(&item_id, ready),
                if ready {
                    "Marked ready."
                } else {
                    "Marked unready."
                },
                cx,
            )
            .is_some()
        {
            self.refresh(cx);
        }
    }

    fn block_click(&mut self, add: bool, cx: &mut Context<Self>) {
        let item_id = Self::read_input(&self.block_target, cx);
        let blocker_id = Self::read_input(&self.block_by, cx);
        if item_id.trim().is_empty() || blocker_id.trim().is_empty() {
            self.set_flash(
                "Blocked item id and blocker item id are required.",
                true,
                cx,
            );
            return;
        }
        if self
            .with_result(
                self.service
                    .set_block_relation(item_id.trim(), blocker_id.trim(), add),
                if add {
                    "Added blocker relation."
                } else {
                    "Removed blocker relation."
                },
                cx,
            )
            .is_some()
        {
            self.refresh(cx);
        }
    }

    fn undo_selected_click(
        &mut self,
        _: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command_id) = self.selected_command_id.clone() else {
            self.set_flash("Select a command first.", true, cx);
            return;
        };
        if self
            .with_result(self.service.undo(&command_id), "Undo applied.", cx)
            .is_some()
        {
            self.refresh(cx);
        }
    }

    fn choose_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        if self
            .with_result(
                self.service.use_project(&project_id),
                "Selected project.",
                cx,
            )
            .is_some()
        {
            self.selected_project_id = Some(project_id);
            self.selected_item_id = None;
            self.selected_command_id = None;
            self.refresh(cx);
        }
    }

    fn choose_item(&mut self, item_id: String, cx: &mut Context<Self>) {
        self.selected_item_id = Some(item_id.clone());
        if let Some(project_id) = self.selected_project_id() {
            match self.service.item_detail(project_id, &item_id) {
                Ok(detail) => {
                    self.sync_edit_inputs(&detail, cx);
                    self.sync_selected_priority(&detail, cx);
                    self.item_detail = Some(detail);
                    cx.notify();
                }
                Err(err) => self.set_flash(err.to_string(), true, cx),
            }
        }
    }

    fn choose_command(&mut self, command_id: String, cx: &mut Context<Self>) {
        self.selected_command_id = Some(command_id.clone());
        match self.service.command_history(&command_id) {
            Ok(detail) => {
                self.command_detail = Some(detail);
                cx.notify();
            }
            Err(err) => self.set_flash(err.to_string(), true, cx),
        }
    }
}

impl Focusable for IssueUi {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn button(id: &'static str, label: &str, highlighted: bool) -> gpui::Stateful<gpui::Div> {
    let mut base = div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(0x2f3541))
        .cursor_pointer()
        .text_sm();
    if highlighted {
        base = base.bg(rgb(0x2f3541)).text_color(rgb(0xf6f7fb));
    } else {
        base = base.bg(rgb(0xffffff)).text_color(rgb(0x1d2330));
    }
    base.hover(|this| this.opacity(0.9))
        .active(|this| this.opacity(0.85))
        .child(label.to_string())
}

fn panel(title: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(rgb(0xf6f7fb))
        .border_1()
        .border_color(rgb(0xd9dde7))
        .rounded_md()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::BOLD)
                .child(title.to_string()),
        )
}

fn metric_card(label: &str, value: usize) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(120.0))
        .p_3()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xd9dde7))
        .rounded_md()
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x586172))
                .child(label.to_string()),
        )
        .child(div().text_xl().child(value.to_string()))
}

impl Render for IssueUi {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overview = self.overview.clone();
        let item_detail = self.item_detail.clone();
        let command_detail = self.command_detail.clone();
        let active_project_id = overview
            .as_ref()
            .and_then(|o| o.active_project.as_ref())
            .map(|p| p.public_id.clone());
        let active_project_name = overview
            .as_ref()
            .and_then(|o| o.active_project.as_ref())
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "No active project".to_string());
        let items = overview
            .as_ref()
            .map(|o| o.items.clone())
            .unwrap_or_default();
        let tree = overview
            .as_ref()
            .map(|o| o.tree.clone())
            .unwrap_or_default();
        let next_items = overview
            .as_ref()
            .map(|o| o.next_items.clone())
            .unwrap_or_default();
        let commands = overview
            .as_ref()
            .map(|o| o.commands.clone())
            .unwrap_or_default();
        let projects = overview
            .as_ref()
            .map(|o| o.projects.clone())
            .unwrap_or_default();
        let create_parent_candidates = self.create_parent_candidates();
        let move_parent_candidates = self.move_parent_candidates();
        let blocker_candidates = self.blocker_candidates();

        let mut root = div()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(rgb(0xeef1f6))
            .text_color(rgb(0x151922))
            .p_4()
            .flex()
            .flex_col()
            .gap_3();

        root = root.child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_2xl().child("issuecli"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x586172))
                                .child(active_project_name),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            button("refresh-button", "Refresh", false)
                                .on_click(cx.listener(Self::refresh_click)),
                        )
                        .child(
                            button("init-button", "Init Repo", false)
                                .on_click(cx.listener(Self::init_click)),
                        ),
                ),
        );

        if let Some(flash) = &self.flash {
            root = root.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(if flash.is_error {
                        rgb(0xffe4e4)
                    } else {
                        rgb(0xe6f7eb)
                    })
                    .border_1()
                    .border_color(if flash.is_error {
                        rgb(0xf2b7b7)
                    } else {
                        rgb(0xb9dfc4)
                    })
                    .child(flash.text.clone()),
            );
        }

        root = root.child(div().flex().gap_3().children([
            metric_card("Projects", projects.len()).into_any_element(),
            metric_card("Items", items.len()).into_any_element(),
            metric_card("Actionable", next_items.len()).into_any_element(),
            metric_card("Recent Commands", commands.len()).into_any_element(),
        ]));

        root = root.child(
            div()
                .flex()
                .gap_3()
                .size_full()
                .child(
                    div()
                        .id("sidebar-scroll")
                        .w(px(300.0))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .overflow_y_scroll()
                        .child(
                            panel("Projects")
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .children(projects.into_iter().enumerate().map(|(ix, project)| {
                                            let project_id = project.public_id.clone();
                                            let is_active = active_project_id.as_deref() == Some(project_id.as_str());
                                            div()
                                                .id(("project", ix))
                                                .px_2()
                                                .py_1()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(0x2f3541))
                                                .cursor_pointer()
                                                .text_sm()
                                                .bg(if is_active { rgb(0x2f3541) } else { rgb(0xffffff) })
                                                .text_color(if is_active { rgb(0xf6f7fb) } else { rgb(0x1d2330) })
                                                .hover(|this| this.opacity(0.9))
                                                .active(|this| this.opacity(0.85))
                                                .child(format!("{} {}", project.public_id, project.name))
                                                .on_click(cx.listener(move |this, _, _, cx| this.choose_project(project_id.clone(), cx)))
                                                .into_any_element()
                                        })),
                                ),
                        )
                        .child(
                            panel("Create Item")
                                .child(self.create_title.clone())
                                .child(self.create_description.clone())
                                .child(section_label("Priority"))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .child(button("create-priority-low", "Low", self.create_priority == PriorityArg::Low).on_click(cx.listener(|this, _, _, cx| this.set_create_priority_click(PriorityArg::Low, cx))))
                                        .child(button("create-priority-medium", "Medium", self.create_priority == PriorityArg::Medium).on_click(cx.listener(|this, _, _, cx| this.set_create_priority_click(PriorityArg::Medium, cx))))
                                        .child(button("create-priority-high", "High", self.create_priority == PriorityArg::High).on_click(cx.listener(|this, _, _, cx| this.set_create_priority_click(PriorityArg::High, cx))))
                                        .child(button("create-priority-urgent", "Urgent", self.create_priority == PriorityArg::Urgent).on_click(cx.listener(|this, _, _, cx| this.set_create_priority_click(PriorityArg::Urgent, cx)))),
                                )
                                .child(self.create_parent.clone())
                                .child(section_label("Parent suggestions"))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .children(create_parent_candidates.into_iter().enumerate().map(|(ix, item)| {
                                            let item_id = item.public_id.clone();
                                            chip(format!("{} {}", item.public_id, item.title), false)
                                                .id(("create-parent-candidate", ix))
                                                .on_click(cx.listener(move |this, _, _, cx| this.choose_create_parent_candidate(item_id.clone(), cx)))
                                                .into_any_element()
                                        })),
                                )
                                .child(button("create-button", "Create", false).on_click(cx.listener(Self::create_click))),
                        )
                        .child(
                            panel("Blockers")
                                .when(item_detail.is_some(), |container| {
                                    let detail = item_detail.clone().expect("detail");
                                    container
                                        .child(field_line("Selected blocked item", detail.item.public_id.clone()))
                                        .child(field_line("Current blockers", joined_or_dash(&detail.blocked_by)))
                                        .child(button("block-selected-target", "Use Selected Item", false).on_click(cx.listener(Self::choose_selected_as_block_target)))
                                })
                                .child(self.block_target.clone())
                                .child(self.block_by.clone())
                                .child(section_label("Blocker suggestions"))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .children(blocker_candidates.into_iter().enumerate().map(|(ix, item)| {
                                            let item_id = item.public_id.clone();
                                            let highlighted = item_detail
                                                .as_ref()
                                                .map(|detail| detail.blocked_by.iter().any(|existing| existing == &item.public_id))
                                                .unwrap_or(false);
                                            chip(format!("{} {}", item.public_id, item.title), highlighted)
                                                .id(("blocker-candidate", ix))
                                                .on_click(cx.listener(move |this, _, _, cx| this.choose_blocker_candidate(item_id.clone(), cx)))
                                                .into_any_element()
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(button("block-button", "Block", false).on_click(cx.listener(|this, _, _, cx| this.block_click(true, cx))))
                                        .child(button("unblock-button", "Unblock", false).on_click(cx.listener(|this, _, _, cx| this.block_click(false, cx)))),
                                ),
                        )
                        .child(
                            panel("Next")
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .children(next_items.into_iter().enumerate().map(|(ix, item)| {
                                            let item_id = item.public_id.clone();
                                            div()
                                                .id(("next-item", ix))
                                                .p_2()
                                                .bg(rgb(0xffffff))
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(0xd9dde7))
                                                .cursor_pointer()
                                                .child(format!("{} [{}] {}", item.public_id, item.priority, item.title))
                                                .on_click(cx.listener(move |this, _, _, cx| this.choose_item(item_id.clone(), cx)))
                                                .into_any_element()
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("items-scroll")
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .overflow_y_scroll()
                        .child(
                            panel("Hierarchy")
                                .child(render_tree_list(&tree, self.selected_item_id.as_deref(), cx)),
                        )
                        .child(
                            panel("All Items")
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .children(items.into_iter().enumerate().map(|(ix, item)| {
                                            let item_id = item.public_id.clone();
                                            let selected = self.selected_item_id.as_deref() == Some(item_id.as_str());
                                            let mut row = div()
                                                .id(("item-row", ix))
                                                .p_2()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(0xd9dde7))
                                                .cursor_pointer()
                                                .child(format!("{} [{} {} ready={}] {}", item.public_id, item.status, item.priority, item.ready, item.title));
                                            row = if selected { row.bg(rgb(0xdfe7ff)) } else { row.bg(rgb(0xffffff)) };
                                            row.on_click(cx.listener(move |this, _, _, cx| this.choose_item(item_id.clone(), cx))).into_any_element()
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("detail-scroll")
                        .w(px(420.0))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .overflow_y_scroll()
                        .child(
                            panel("Selected Item")
                                .when(item_detail.is_some(), |container| {
                                    let detail = item_detail.clone().expect("detail");
                                    container.child(detail_header(&detail))
                                        .child(section_label("Edit"))
                                        .child(self.edit_title.clone())
                                        .child(self.edit_description.clone())
                                        .child(section_label("Priority"))
                                        .child(
                                            div()
                                                .flex()
                                                .gap_2()
                                                .flex_wrap()
                                                .child(button("edit-priority-low", "Low", self.edit_priority == PriorityArg::Low).on_click(cx.listener(|this, _, _, cx| this.set_edit_priority_click(PriorityArg::Low, cx))))
                                                .child(button("edit-priority-medium", "Medium", self.edit_priority == PriorityArg::Medium).on_click(cx.listener(|this, _, _, cx| this.set_edit_priority_click(PriorityArg::Medium, cx))))
                                                .child(button("edit-priority-high", "High", self.edit_priority == PriorityArg::High).on_click(cx.listener(|this, _, _, cx| this.set_edit_priority_click(PriorityArg::High, cx))))
                                                .child(button("edit-priority-urgent", "Urgent", self.edit_priority == PriorityArg::Urgent).on_click(cx.listener(|this, _, _, cx| this.set_edit_priority_click(PriorityArg::Urgent, cx)))),
                                        )
                                        .child(section_label("Parent"))
                                        .child(field_line("Current parent", detail.item.parent_id.clone().unwrap_or_else(|| "Root".to_string())))
                                        .child(self.move_parent.clone())
                                        .child(
                                            div()
                                                .flex()
                                                .gap_2()
                                                .flex_wrap()
                                                .child(button("move-root-button", "Move To Root", false).on_click(cx.listener(Self::choose_root_parent)))
                                                .children(move_parent_candidates.into_iter().enumerate().map(|(ix, item)| {
                                                    let item_id = item.public_id.clone();
                                                    let highlighted = detail.item.parent_id.as_deref() == Some(item.public_id.as_str());
                                                    chip(format!("{} {}", item.public_id, item.title), highlighted)
                                                        .id(("move-parent-candidate", ix))
                                                        .on_click(cx.listener(move |this, _, _, cx| this.choose_move_parent_candidate(item_id.clone(), cx)))
                                                        .into_any_element()
                                                })),
                                        )
                                        .child(section_label("Actions"))
                                        .child(
                                            div()
                                                .flex()
                                                .gap_2()
                                                .flex_wrap()
                                                .child(button("save-button", "Save", false).on_click(cx.listener(Self::save_item_click)))
                                                .child(button("move-button", "Move", false).on_click(cx.listener(Self::move_item_click)))
                                                .child(button("ready-button", "Ready", false).on_click(cx.listener(|this, _, _, cx| this.set_ready_click(true, cx))))
                                                .child(button("unready-button", "Unready", false).on_click(cx.listener(|this, _, _, cx| this.set_ready_click(false, cx))))
                                                .child(button("todo-button", "Todo", false).on_click(cx.listener(|this, _, _, cx| this.set_status_click(StatusArg::Todo, cx))))
                                                .child(button("progress-button", "In Progress", false).on_click(cx.listener(|this, _, _, cx| this.set_status_click(StatusArg::InProgress, cx))))
                                                 .child(button("done-button", "Done", false).on_click(cx.listener(|this, _, _, cx| this.set_status_click(StatusArg::Done, cx))))
                                                 .child(button("cancel-button", "Cancel", false).on_click(cx.listener(|this, _, _, cx| this.set_status_click(StatusArg::Cancelled, cx)))),
                                        )
                                        .child(section_label("Relationships"))
                                        .child(field_line("Children", join_item_labels(&detail.children)))
                                        .child(field_line("Blocked by", joined_or_dash(&detail.blocked_by)))
                                        .child(field_line("Blocks", joined_or_dash(&detail.blockers)))
                                        .child(section_label("Recent item events"))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .children(detail.history.iter().rev().take(8).map(|event| {
                                                    div().text_sm().text_color(rgb(0x586172)).child(format!("{} {}", event.operation, event.created_at)).into_any_element()
                                                })),
                                        )
                                })
                                .when(item_detail.is_none(), |container| container.child(div().text_sm().text_color(rgb(0x586172)).child("Select an item to inspect and edit."))),
                        )
                        .child(
                            panel("History")
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .children(commands.into_iter().enumerate().map(|(ix, command)| {
                                            let command_id = command.public_id.clone();
                                            let selected = self.selected_command_id.as_deref() == Some(command_id.as_str());
                                            let mut row = div()
                                                .id(("command-row", ix))
                                                .p_2()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(0xd9dde7))
                                                .cursor_pointer()
                                                .child(format!("{} {}", command.public_id, command.action));
                                            row = if selected { row.bg(rgb(0xdfe7ff)) } else { row.bg(rgb(0xffffff)) };
                                            row.on_click(cx.listener(move |this, _, _, cx| this.choose_command(command_id.clone(), cx))).into_any_element()
                                        })),
                                )
                                .child(button("undo-button", "Undo Selected", false).on_click(cx.listener(Self::undo_selected_click)))
                                .when(command_detail.is_some(), |container| {
                                    let (command, events) = command_detail.clone().expect("command detail");
                                    container.child(div().text_sm().child(format!("{} {}", command.public_id, command.action)))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .children(events.into_iter().map(|event| {
                                                    div().text_sm().text_color(rgb(0x586172)).child(format!("{} {} {}", event.entity_type, event.operation, event.entity_key)).into_any_element()
                                                })),
                                        )
                                }),
                        ),
                ),
        );

        root
    }
}

fn section_label(text: &str) -> gpui::Div {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(0x586172))
        .child(text.to_string())
}

fn field_line(label: &str, value: String) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section_label(label))
        .child(div().text_sm().child(value))
}

fn chip(label: String, highlighted: bool) -> gpui::Div {
    let mut chip = div()
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(0xd9dde7))
        .cursor_pointer()
        .text_xs();
    chip = if highlighted {
        chip.bg(rgb(0xdfe7ff)).text_color(rgb(0x1d2330))
    } else {
        chip.bg(rgb(0xffffff)).text_color(rgb(0x1d2330))
    };
    chip.hover(|this| this.opacity(0.9)).child(label)
}

fn detail_header(detail: &ItemDetail) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_lg()
                .child(format!("{} {}", detail.item.public_id, detail.item.title)),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .flex_wrap()
                .child(chip(format!("status {}", detail.item.status), false))
                .child(chip(format!("priority {}", detail.item.priority), false))
                .child(chip(
                    format!("ready {}", detail.item.ready),
                    detail.item.ready,
                )),
        )
        .child(div().text_sm().text_color(rgb(0x586172)).child(
            if detail.item.description.is_empty() {
                "No description".to_string()
            } else {
                detail.item.description.clone()
            },
        ))
}

fn joined_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(", ")
    }
}

fn join_item_labels(items: &[crate::domain::WorkItemRecord]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("{} {}", item.public_id, item.title))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn push_candidate(
    candidates: &mut Vec<crate::domain::WorkItemRecord>,
    seen: &mut HashSet<String>,
    item: &crate::domain::WorkItemRecord,
) {
    if seen.insert(item.public_id.clone()) {
        candidates.push(item.clone());
    }
}

fn push_candidate_if_allowed(
    candidates: &mut Vec<crate::domain::WorkItemRecord>,
    seen: &mut HashSet<String>,
    blocked: &HashSet<String>,
    item: &crate::domain::WorkItemRecord,
) {
    if !blocked.contains(&item.public_id) {
        push_candidate(candidates, seen, item);
    }
}

fn collect_descendant_ids(
    tree: &[TreeNode],
    selected_id: &str,
    blocked: &mut HashSet<String>,
) -> bool {
    for node in tree {
        if node.item.public_id == selected_id {
            collect_tree_ids(&node.children, blocked);
            return true;
        }
        if collect_descendant_ids(&node.children, selected_id, blocked) {
            return true;
        }
    }
    false
}

fn collect_tree_ids(tree: &[TreeNode], blocked: &mut HashSet<String>) {
    for node in tree {
        blocked.insert(node.item.public_id.clone());
        collect_tree_ids(&node.children, blocked);
    }
}

fn render_tree_list(
    tree: &[TreeNode],
    selected_item_id: Option<&str>,
    cx: &mut Context<IssueUi>,
) -> impl IntoElement {
    let mut rows = Vec::new();
    for node in tree {
        push_tree_rows(&mut rows, node, 0, selected_item_id, cx);
    }
    div().flex().flex_col().gap_2().children(rows)
}

fn push_tree_rows(
    rows: &mut Vec<gpui::AnyElement>,
    node: &TreeNode,
    depth: usize,
    selected_item_id: Option<&str>,
    cx: &mut Context<IssueUi>,
) {
    let item_id = node.item.public_id.clone();
    let selected = selected_item_id == Some(item_id.as_str());
    let mut row = div()
        .id(("tree-row", rows.len()))
        .pl(px((depth as f32) * 16.0 + 8.0))
        .pr_2()
        .py_2()
        .rounded_sm()
        .border_1()
        .border_color(rgb(0xd9dde7))
        .cursor_pointer()
        .child(format!(
            "{} [{} ready={}] {}",
            node.item.public_id, node.item.status, node.item.ready, node.item.title
        ));
    row = if selected {
        row.bg(rgb(0xdfe7ff))
    } else {
        row.bg(rgb(0xffffff))
    };
    rows.push(
        row.on_click(cx.listener(move |this, _, _, cx| this.choose_item(item_id.clone(), cx)))
            .into_any_element(),
    );
    for child in &node.children {
        push_tree_rows(rows, child, depth + 1, selected_item_id, cx);
    }
}

use std::fmt::{self, Display};

use anyhow::Result;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusArg {
    Todo,
    InProgress,
    Done,
    Cancelled,
}

impl StatusArg {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

impl Display for StatusArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for StatusArg {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "todo" => Ok(Self::Todo),
            "in_progress" => Ok(Self::InProgress),
            "done" => Ok(Self::Done),
            "cancelled" => Ok(Self::Cancelled),
            _ => anyhow::bail!("invalid status: {s}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PriorityArg {
    Low,
    Medium,
    High,
    Urgent,
}

impl PriorityArg {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

impl Display for PriorityArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PriorityArg {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "urgent" => Ok(Self::Urgent),
            _ => anyhow::bail!("invalid priority: {s}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub public_id: String,
    pub name: String,
    pub repo_root: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemRecord {
    pub public_id: String,
    pub project_id: i64,
    pub title: String,
    pub description: String,
    pub ready: bool,
    pub status: String,
    pub priority: String,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRecord {
    pub public_id: String,
    pub project_id: Option<String>,
    pub action: String,
    pub actor: String,
    pub undone_command_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub entity_type: String,
    pub entity_key: String,
    pub operation: String,
    pub before_state: Option<Value>,
    pub after_state: Option<Value>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    pub item: WorkItemRecord,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: i64,
    pub record: ProjectRecord,
}

#[derive(Debug, Clone)]
pub struct WorkItemRow {
    pub row_id: i64,
    pub record: WorkItemRecord,
}

#[derive(Debug, Clone)]
pub struct InternalCommandRecord {
    pub id: i64,
    pub public_id: String,
    pub project_id: Option<i64>,
    pub action: String,
}

#[derive(Debug, Clone)]
pub struct ItemListFilter {
    pub status: Option<StatusArg>,
    pub priority: Option<PriorityArg>,
    pub ready: Option<bool>,
    pub blocked: Option<bool>,
    pub parent: Option<String>,
    pub root: bool,
}

pub fn bool_to_i64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

pub fn work_item_state(item: &WorkItemRecord) -> Value {
    json!(item)
}

pub fn work_item_event_key(project_id: i64, item_id: &str) -> String {
    format!("{project_id}:{item_id}")
}

pub fn blocker_relation_event_key(project_id: i64, blocker_id: &str, blocked_id: &str) -> String {
    format!("{project_id}:{blocker_id}->{blocked_id}")
}

pub fn parse_json(input: &str) -> Option<Value> {
    serde_json::from_str(input).ok()
}

pub fn work_item_from_value(value: &Value) -> anyhow::Result<WorkItemRecord> {
    serde_json::from_value(value.clone()).map_err(Into::into)
}

pub fn join_ids(items: &[String]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(",")
    }
}

pub fn join_item_ids(items: &[WorkItemRecord]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items
            .iter()
            .map(|item| item.public_id.clone())
            .collect::<Vec<_>>()
            .join(",")
    }
}

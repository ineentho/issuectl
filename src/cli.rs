use clap::{Args, Parser, Subcommand};

use crate::domain::{PriorityArg, StatusArg};

#[derive(Parser, Debug)]
#[command(name = "issuecli")]
#[command(about = "Local work-item tracking for repository workflows")]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Init,
    Project(ProjectArgs),
    Item(ItemArgs),
    Review(ReviewArgs),
    Next(NextArgs),
    History(HistoryArgs),
    Undo(UndoArgs),
    Ui,
}

#[derive(Args, Debug)]
pub struct ReviewArgs {
    #[command(subcommand)]
    pub command: ReviewCommand,
}

#[derive(Subcommand, Debug)]
pub enum ReviewCommand {
    Tree { item_id: Option<String> },
}

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    Show,
    List,
    Use { project_id: String },
}

#[derive(Args, Debug)]
pub struct ItemArgs {
    #[command(subcommand)]
    pub command: ItemCommand,
}

#[derive(Subcommand, Debug)]
pub enum ItemCommand {
    Create(ItemCreateArgs),
    List(ItemListArgs),
    Show {
        item_id: String,
    },
    Update(ItemUpdateArgs),
    Status {
        item_id: String,
        status: StatusArg,
    },
    Ready {
        item_id: String,
    },
    Unready {
        item_id: String,
    },
    Block {
        item_id: String,
        #[arg(long = "by")]
        blocker_id: String,
    },
    Unblock {
        item_id: String,
        #[arg(long = "by")]
        blocker_id: String,
    },
    Blockers {
        item_id: String,
    },
    Move(ItemMoveArgs),
    Children {
        item_id: String,
    },
    Tree {
        item_id: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct ItemCreateArgs {
    #[arg(long)]
    pub title: String,
    #[arg(long, default_value = "")]
    pub description: String,
    #[arg(long, value_enum, default_value_t = PriorityArg::Medium)]
    pub priority: PriorityArg,
    #[arg(long)]
    pub parent: Option<String>,
}

#[derive(Args, Debug)]
pub struct ItemListArgs {
    #[arg(long, value_enum)]
    pub status: Option<StatusArg>,
    #[arg(long, value_enum)]
    pub priority: Option<PriorityArg>,
    #[arg(long)]
    pub ready: Option<bool>,
    #[arg(long)]
    pub blocked: Option<bool>,
    #[arg(long)]
    pub parent: Option<String>,
    #[arg(long)]
    pub root: bool,
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Args, Debug)]
pub struct ItemUpdateArgs {
    pub item_id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, value_enum)]
    pub status: Option<StatusArg>,
    #[arg(long, value_enum)]
    pub priority: Option<PriorityArg>,
    #[arg(long)]
    pub parent: Option<String>,
    #[arg(long)]
    pub root: bool,
}

#[derive(Args, Debug)]
pub struct ItemMoveArgs {
    pub item_id: String,
    #[arg(long)]
    pub parent: Option<String>,
    #[arg(long)]
    pub root: bool,
}

#[derive(Args, Debug)]
pub struct NextArgs {
    #[arg(long, default_value_t = 1)]
    pub limit: usize,
    #[arg(long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub struct HistoryArgs {
    #[command(subcommand)]
    pub command: HistoryCommand,
}

#[derive(Subcommand, Debug)]
pub enum HistoryCommand {
    Show { item_id: String },
    Command { command_id: String },
    List,
}

#[derive(Args, Debug)]
pub struct UndoArgs {
    pub command_id: String,
}

impl From<&ItemListArgs> for crate::domain::ItemListFilter {
    fn from(value: &ItemListArgs) -> Self {
        Self {
            status: value.status,
            priority: value.priority,
            ready: value.ready,
            blocked: value.blocked,
            parent: value.parent.clone(),
            root: value.root,
        }
    }
}

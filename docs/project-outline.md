# Project Outline

## Summary

This project is a local, command-line-first tool for managing project work items and related workflow.

It is designed primarily for LLM-driven usage on a local machine, with a simple interface and predictable behavior. Conceptually, it should feel like a very basic local version of Linear, without collaboration or cloud features.

## Core Product Direction

- Local-first
- CLI-first
- Designed for LLM use
- Simple work-item management
- Reversible operations with full history
- No sync or remote collaboration

## Non-Goals

- No cloud backend
- No multi-user collaboration
- No remote sync
- No complex project-management feature set

## Initial Product Idea

The tool should let a user or local LLM create, list, update, and organize work items from the command line.

The main value is giving LLMs a structured, reliable, local system for tracking work in a project without relying on external services.

## Likely Core Entities

- Work items
- Status
- Priority
- Projects or groups

Issues and tasks should not be modeled as different concepts in the initial design. They are the same underlying entity and should be represented as a single work-item model.

## Initial Design Principles

- Fast local operations
- Human-readable command outputs
- Predictable machine-friendly output for LLMs
- Minimal setup
- Easy to script
- Storage kept simple and local
- Mutations should be reversible
- Concurrency behavior should be explicit and safe

## Storage Direction

The system should use a local embedded database, with SQLite as the default starting point unless a similar option offers materially better guarantees for local durability and locking.

The storage layer should remain self-contained and easy to back up, inspect, and move between local environments.

SQLite should be treated as the source of truth, with a schema that separates current state from immutable history.

The MVP should use a single global database stored in the user's home directory rather than a separate database per repository.

Projects should be resolved within that global database through repository-to-project association.

The default database path should be `~/.issuecli/db.sqlite3`.

## History And Reversibility

The system should keep a full local history of all state changes so that actions are auditable and reversible.

This strongly suggests an append-only event log or equivalent durable history model, rather than only storing the latest row state.

Key requirements:

- Every mutation should leave a durable history entry
- Past state should be reconstructible
- Revert or undo operations should be a first-class workflow
- History should remain local and available offline
- The model should be reliable for LLM-driven automation, where accidental edits must be recoverable

The preferred model is compensating events rather than deleting or mutating history. Undo should create new history entries that restore prior state while preserving the full audit trail.

## Locking And Concurrency

The system needs robust locking and write coordination across processes, including cases where the same database is accessed across filesystem boundaries such as mounted Docker volumes.

This means concurrency strategy must be treated as a core architectural concern rather than an implementation detail.

Key requirements:

- Safe concurrent access from multiple local processes
- Clear single-writer or otherwise well-defined mutation semantics
- Locking behavior that remains reliable in containerized and mounted-volume setups
- Recovery behavior for interrupted writes or stale locks
- A design that does not assume a single in-memory process owns the state

The initial implementation should prefer single-writer semantics for mutations, while allowing concurrent reads. This keeps behavior predictable, reduces conflict complexity, and makes undo safer.

## Proposed Storage Architecture

The storage model should have three layers:

- Current-state tables for fast reads
- Append-only history tables for all mutations
- Coordination tables for write ownership and command tracking

This allows fast CLI queries while preserving a complete local audit trail.

### Current-State Tables

These tables represent the latest materialized state and are the main query surface for the CLI.

Likely tables:

- `work_items`
- `projects`

Each mutable record should include a monotonically increasing version field to support validation, debugging, and optimistic checks.

## MVP Work-Item Schema

The MVP work-item model should stay small and opinionated.

Recommended fields:

- `id`
- `title`
- `description`
- `ready`
- `status`
- `priority`
- `project_id`
- `parent_id`
- `created_at`
- `updated_at`
- `closed_at`
- `version`

The model should not include separate `kind` or `assignee` fields in the MVP.

Work-item ids should use a short Linear-style format such as `WI-12`.

The numeric portion should be a per-project sequence rather than a single global sequence.

`ready` should be a required boolean field that defaults to `false`.

This field represents explicit human review and approval for implementation readiness. Since work items may be created or modified by agents, they should not be considered ready to implement until a human marks them as ready.

Only explicit human actions should change `ready`. Agent edits should not automatically toggle it.

This makes human review a built-in workflow gate rather than an informal convention.

### Hierarchy

Work items should support explicit parent-child relationships through `parent_id`.

This should make the issue model tree-shaped rather than flat, allowing larger pieces of work to be decomposed into nested child items.

The initial hierarchy model should remain simple:

- Each work item may have zero or one parent
- Each work item may have many children
- Parent-child relationships should stay within the same project
- CLI workflows should support traversing both upward to parents and downward to children
- Parent-child cycles should be forbidden

This should be treated as a core concept in the product rather than an optional extension.

### Blocking Relations

Work items should also support explicit blocking relationships that are separate from the parent-child tree.

Parent-child structure models decomposition. Blocking relations model execution dependencies.

The MVP should support a many-to-many dependency model where one work item can block many other items, and one work item can be blocked by many other items.

Recommended representation:

- A dedicated relation such as `work_item_blockers`
- `blocker_id`
- `blocked_id`
- `created_at`

Important behavior:

- Blocking relations should not be inferred automatically from hierarchy
- A child item is not automatically blocked by its parent unless an explicit blocking relation exists
- An item is actionable only when it is ready and not blocked by any non-terminal work item
- Blocking cycles should be forbidden

This separation is important because tree structure and execution order are related but not identical concepts.

### Recommended MVP Statuses

- `todo`
- `in_progress`
- `done`
- `cancelled`

Status transitions should be freely allowed between supported statuses in the MVP.

`done` and `cancelled` should be treated as terminal states everywhere in the system.

`closed_at` should be set when a work item enters a terminal state and cleared when it returns to a non-terminal state.

### Recommended MVP Priorities

- `low`
- `medium`
- `high`
- `urgent`

## Project Model

Projects should be a first-class concept in the MVP.

The default and most common model should be one project linked to one Git repository.

This means the typical workflow is:

1. A user initializes issuecli inside a repository.
2. The tool creates or links to a single local project for that repository.
3. Work items created in that repository belong to that project by default.

This default should keep the CLI simple for both humans and LLMs, because project selection can usually be inferred from the current working tree.

### Repository Association

Projects should support an explicit association with a local Git repository.

Recommended project fields:

- `id`
- `name`
- `repo_path`
- `repo_root`
- `created_at`
- `updated_at`
- `version`

The exact repository metadata can be refined later, but the important MVP behavior is that a project can be resolved from the current repository context.

The project itself should live in the global home-directory database even when it is associated with a specific repository.

Project identity should be derived from the repository worktree root.

The implementation should resolve the worktree root explicitly and normalize the original directory through symlink resolution so the canonical project anchor is a resolved absolute repository root path.

### Default Project Behavior

Preferred default behavior:

- Running the CLI inside a Git repository should resolve the active project automatically
- New work items should attach to the active project by default
- Commands should not require an explicit project argument in the common single-repository case
- Explicit project selection should still be possible when needed

If the current working directory is not inside a Git repository, commands that require an active project should fail clearly rather than guessing.

### Scope Boundaries

The initial design should optimize for one project per repository, not for a large multi-project workspace.

Multiple projects in one database may still be supported, but that should be secondary to the repository-linked default workflow.

### History Tables

History should be append-only and operation-based rather than just snapshot-based.

Likely tables:

- `work_item_events`
- `work_item_blocker_events`
- `project_events`
- `commands` or `operations`

Each event should capture enough data to understand what happened and support reversal.

Representative event fields:

- `id`
- `entity_type`
- `entity_id`
- `operation`
- `payload`
- `inverse_payload` or equivalent reversal data
- `actor`
- `created_at`
- `command_id`

`command_id` is important because one CLI command may touch multiple entities. Undo should usually operate on the whole command, not only a single row update.

### Coordination Tables

The database should also hold coordination state used to serialize writes and recover from interrupted processes.

Likely table:

- `locks`

Representative lock fields:

- `lock_name`
- `owner_id`
- `leased_until`
- `heartbeat_at`

Lock ownership should be token-based so one process cannot accidentally release another process's lock.

## Write Model

Every mutating CLI command should execute as a single transaction.

Preferred flow:

1. Acquire write coordination.
2. Read current state and version.
3. Validate preconditions.
4. Insert command or operation record.
5. Insert append-only history events.
6. Update current-state tables.
7. Commit.

If the transaction fails, no partial change should be visible.

This means current state and history stay consistent with each other by construction.

## Undo Model

Undo should be a first-class command and should operate primarily through compensating events.

Preferred behavior:

1. Look up the original `command_id`.
2. Determine the affected entities and inverse data.
3. Write new reversal events.
4. Update current-state tables in the same transaction.

This preserves a complete audit history instead of rewriting or deleting the past.

The initial design should support:

- Undo of a whole command
- Reconstruction of prior state from history

Point-in-time restore may be added later, but command-level undo is the most useful starting point.

Undo should refuse unsafe reversals. In the MVP, a reversal is unsafe if a later command has modified the same entity.

## Concurrency Strategy

The system should not rely only on filesystem lock files. Those can become unreliable or subtle across container boundaries, mounted volumes, or unusual filesystem setups.

Preferred approach:

- Use SQLite transaction semantics as the primary database guard
- Use a database-backed lease for explicit write ownership
- Keep write transactions short
- Allow many readers and one writer

This design keeps coordination close to the data and avoids assuming a single long-lived in-memory process owns the state.

### SQLite Locking Expectations

SQLite's built-in locking should be treated as the low-level concurrency primitive for the system.

It is a good fit for the MVP because it already provides:

- Atomic transactions
- Safe many-reader, single-writer behavior
- OS-level file locking between local processes
- Good local concurrency characteristics when used in WAL mode

For normal local filesystems and multiple local processes on one machine, this should generally be reliable.

However, SQLite locking should not be treated as the entire coordination story for this product. Filesystem behavior can vary across mounted volumes, container boundaries, and non-standard filesystem layers.

This means the design should rely on SQLite for transactional correctness, but still use an application-level database lease for clearer write ownership, stale-writer recovery, and more predictable failure handling.

Practical implications:

- Use SQLite in WAL mode
- Keep write transactions short
- Configure sensible busy-timeout or retry behavior
- Treat the app-level lease as the user-facing write-coordination mechanism
- Validate mounted-volume and container scenarios explicitly rather than assuming they are safe

The intended model is therefore:

- SQLite locking for low-level correctness
- Application lease logic for higher-level coordination semantics

## Crash Recovery

Crash recovery should be simple and deterministic.

Required behavior:

- Detect expired leases
- Prevent partial writes by keeping mutations transactional
- Mark interrupted commands as failed or incomplete if needed
- Trust only committed history and committed current state

If all mutations happen in a single transaction, recovery remains straightforward.

## Recommended MVP Architecture

The recommended first implementation is:

- SQLite in WAL mode
- Current-state tables for work items and related entities
- A dedicated blocking-relation table for work-item dependencies
- Append-only event tables
- A `commands` or `operations` table for grouping related mutations
- A database-backed lease for single-writer coordination
- Undo implemented with compensating events

This is the smallest architecture that still provides local-first storage, strong reversibility, and safer LLM-oriented automation.

## Command-Line Interface

The CLI should be simple, explicit, and easy for both humans and LLMs to drive.

The MVP should favor a small number of predictable commands over a large feature surface.

### CLI Design Principles

- Use resource-oriented commands
- Keep names short and predictable
- Prefer explicit flags over interactive prompts
- Make the common repository-local workflow require minimal arguments
- Support both human-readable and machine-readable output
- Return stable identifiers and stable field names

### Primary Command Groups

- `init`
- `project`
- `item`
- `next`
- `history`
- `undo`

`item` should be the main command group for work-item management.

### Repository Initialization

Recommended command:

- `issuecli init`

Expected behavior:

- Initialize local issuecli state for the current repository
- Create or connect the local database
- Create the default project for the repository if one does not exist
- Record the repository-to-project association

This should be the normal entry point for a new repository.

### Project Commands

Recommended MVP commands:

- `issuecli project show`
- `issuecli project list`
- `issuecli project use <project-id>`

Expected behavior:

- `project show` displays the active project resolved from the current repository context
- `project list` lists known local projects
- `project use` allows explicit project selection when auto-resolution is not enough

In the common one-project-per-repository case, most users should rarely need explicit project commands after initialization.

### Work-Item Commands

Recommended MVP commands:

- `issuecli item create`
- `issuecli item list`
- `issuecli item show <item-id>`
- `issuecli item update <item-id>`
- `issuecli item status <item-id> <status>`
- `issuecli item ready <item-id>`
- `issuecli item unready <item-id>`
- `issuecli item block <item-id> --by <blocker-id>`
- `issuecli item unblock <item-id> --by <blocker-id>`
- `issuecli item blockers <item-id>`
- `issuecli item move <item-id> --parent <parent-id>`
- `issuecli item move <item-id> --root`
- `issuecli item children <item-id>`

Expected responsibilities:

- `item create` creates a new work item in the active project, defaulting `ready=false`
- `item list` lists work items, usually scoped to the active project
- `item show` displays a single work item with parent and child context
- `item update` edits mutable fields such as title, description, status, priority, or parent
- `item status` changes the item status to any supported status and updates `closed_at` when entering or leaving a terminal state
- `item ready` marks a work item as human-reviewed and ready for implementation
- `item unready` clears readiness if the work item needs further review or revision
- `item block` adds a blocking dependency from one work item to another
- `item unblock` removes a blocking dependency
- `item blockers` lists blocking and blocked-by relations for an item
- `item move` changes hierarchy placement
- `item children` lists direct child items

### Tree And Navigation Commands

Because hierarchy is a core concept, the CLI should support tree-oriented views directly.

Recommended MVP commands:

- `issuecli item tree`
- `issuecli item tree <item-id>`

Expected behavior:

- Without an item id, show the project-level work-item tree
- With an item id, show that item and its descendants
- Render parent-child structure clearly in human output
- Preserve stable structure in machine-readable output

### Next-Work Command

The CLI should include a command that selects the next work items that are ready to implement.

Recommended MVP command:

- `issuecli next`

Expected behavior:

- Select candidate work items from the active project
- Exclude work items that are not `ready`
- Exclude work items in terminal states such as `done` or `cancelled`
- Exclude work items blocked by any non-terminal blocker
- Exclude work items that have any non-terminal child items
- Sort remaining candidates in a predictable order

### Actionable Semantics

For the MVP, `issuecli next` should return only actionable work items.

A work item is actionable only if all of the following are true:

- It belongs to the active project
- `ready=true`
- Its status is not terminal
- It is not blocked by any work item whose status is not terminal
- It has no child work items whose status is not terminal

This means `next` should prefer executable leaf work by rule, not by heuristic.

In practice:

- Parent items that still have open children are planning or coordination items and should not be returned by `next`
- A parent item may be returned only when all of its children are terminal, or when it has no children
- Closed blockers should not block anything
- Cancelled children should count as terminal for purposes of `next`

These semantics make `next` deterministic and keep agents focused on directly implementable work.

Recommended default sort order:

1. Highest priority first
2. Oldest ready items first
3. Oldest created items first
4. Stable tie-break by id

The sort order can evolve later, but the actionable filter should remain explicit and deterministic.

### Suggested Optional Flags For `next`

The base `issuecli next` command should remain strict and simple, but a few optional flags may be useful:

- `--limit <n>` to return more than one actionable item
- `--json` to support agent consumption

The default behavior should return exactly one next work item. `--limit <n>` should expand that result set when needed.

If no actionable work item exists, the command should print a human-readable message such as `No unblocked work items are available.` and exit with a non-zero return code.

This command is especially important for LLM and agent workflows because it provides a canonical way to discover actionable work without relying on custom heuristics.

### History Commands

Recommended MVP commands:

- `issuecli history show <item-id>`
- `issuecli history command <command-id>`
- `issuecli history list`

Expected behavior:

- `history show` lists the event history for a work item
- `history command` shows the full set of changes made by one command
- `history list` lists recent commands or operations for the active project

This should make local audit and debugging straightforward.

### Undo Commands

Recommended MVP commands:

- `issuecli undo <command-id>`

Expected behavior:

- Reverse an earlier command by writing compensating events
- Reject undo if the reversal cannot be applied safely
- Record the undo itself as a new command in history

Undo should target commands rather than arbitrary raw row changes.

### Filtering And Selection

The MVP CLI should support a small set of filters on list-style commands.

Recommended filters:

- `--status`
- `--priority`
- `--ready`
- `--blocked`
- `--parent`
- `--root`
- `--project`

This is enough to support common human and agent workflows without introducing a full query language in the MVP.

### Output Modes

The CLI should support both default human-readable output and explicit machine-readable output.

Recommended approach:

- Human-readable output by default
- `--json` for machine-readable output

Important expectations for `--json`:

- Stable field names
- Stable top-level shapes per command
- No mixed prose in JSON mode
- Sufficient data to avoid requiring screen-scraping by agents
- Errors should also use a stable JSON shape in JSON mode

### Exit Codes

The CLI should use stable return codes so it is easy to drive from scripts and agents.

Recommended MVP behavior:

- `0` for success
- `1` for general operational or validation errors
- `2` for usage errors such as invalid arguments
- `3` for expected empty-result conditions where the command ran correctly but no actionable result exists, such as `issuecli next` finding no unblocked ready work

The exact mapping can evolve if needed, but the MVP should keep these meanings stable.

### Identifier Formats

The CLI should use short, stable uppercase identifiers for primary user-facing entities.

Recommended MVP formats:

- Work items: `WI-12`
- Commands: `CMD-104`

Work-item ids should be per-project sequences. Command ids may be globally sequenced within the single global database.

### Suggested Command Examples

```bash
issuecli init
issuecli item create --title "Add local DB lease handling"
issuecli item create --title "Test Docker volume locking" --parent WI-12
issuecli item block WI-18 --by WI-12
issuecli item list --status todo --ready false
issuecli item ready WI-12
issuecli item status WI-12 in_progress
issuecli next
issuecli next --limit 5
issuecli item tree
issuecli history show WI-12
issuecli undo CMD-104
```

### MVP CLI Notes

The command set should optimize for these default assumptions:

- One active project resolved from the current Git repository
- One primary work-item type
- Parent-child hierarchy as a normal workflow
- Explicit dependency tracking for blocked work
- A canonical `next` command for finding actionable work
- Human review required before implementation through `ready`
- Auditability and reversibility built into all mutations

## Risks And Validation Needs

SQLite is the preferred starting point, but locking and mounted-volume behavior should be explicitly tested in realistic environments, including Docker volume scenarios.

If SQLite locking semantics prove insufficient for the target environments, a later fallback could be a local server process that owns all writes while still keeping the system local-first.

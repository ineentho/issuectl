---
name: issuectl-cli
description: Use the issuectl command-line interface to inspect and update local work-item projects, including projects, items, blockers, next-item selection, history, undo, and review workflows.
allowed-tools: Bash(issuectl:*)
---

# issuectl CLI

Use this skill when the task is about inspecting or changing data managed by the `issuectl` CLI.

Prefer the CLI over direct SQLite access. The CLI already enforces project resolution, history recording, undo semantics, and validation rules.

## Core rules

- Run `issuectl --json ...` whenever the output will be parsed or used to drive follow-up decisions.
- Use human-readable output only when the user explicitly wants terminal-facing text.
- Do not write to the SQLite database directly.
- Prefer narrow commands over broad manual inspection. Example: use `issuectl item show WI-7` instead of grepping the database.
- Be careful with `undo`: it is command-level undo, not arbitrary entity rollback.
- `issuectl next` returns exit code `3` when no actionable items exist. Treat that as an empty result, not a crash.

## Project model

`issuectl` is repo-aware.

- `issuectl init`
  Creates or resolves the project for the current Git repository.
- `issuectl project show`
  Show the active project.
- `issuectl project show --explain`
  Show the active project and why it was selected.
- `issuectl project list`
  List known projects.
- `issuectl project use PRJ-1`
  Switch the active project override.

If work is happening in a repository, start with `issuectl --json project show --explain` or `issuectl --json init`.

Project resolution precedence:

1. An explicit `--project <PROJECT_ID>` on supported write commands.
2. The active project override selected via `project use`.
3. The current Git repository root.

## Items

### Create

```bash
issuectl --json item create --title "Add caching" --description "Cache issue lookups" --priority high
issuectl --json item create --project PRJ-2 --title "Refactor parser" --parent APP-12
```

### List and inspect

```bash
issuectl --json item list
issuectl --json item list --status todo --ready false
issuectl --json item list --blocked true
issuectl --json item list --project PRJ-2
issuectl --json item show APP-12
issuectl --json item children APP-12
issuectl --json item tree
issuectl --json review tree
```

Useful list filters:

- `--status todo|in-progress|done|cancelled`
- `--priority low|medium|high|urgent`
- `--ready true|false`
- `--blocked true|false`
- `--parent <ITEM_ID>`
- `--root`
- `--project <PROJECT_ID>`

### Update

Use `item update` for multi-field changes.

```bash
issuectl --json item update APP-12 --title "Add SQLite caching" --priority urgent
issuectl --json item update APP-12 --project PRJ-2 --parent APP-4
issuectl --json item update APP-12 --root
```

Use focused commands for common state changes.

```bash
issuectl --json item status APP-12 in-progress
issuectl --json item ready APP-12
issuectl --json item unready APP-12
issuectl --json item move APP-12 --parent APP-4
issuectl --json item block APP-12 --by APP-9
issuectl --json item unblock APP-12 --by APP-9
```

Mutating item commands support explicit project targeting:

- `item create --project <PROJECT_ID>`
- `item update --project <PROJECT_ID>`
- `item move --project <PROJECT_ID>`
- `item ready --project <PROJECT_ID>`
- `item unready --project <PROJECT_ID>`
- `item status --project <PROJECT_ID>`
- `item block --project <PROJECT_ID>`
- `item unblock --project <PROJECT_ID>`

## Review and next-item workflow

Use `next` to find actionable work.

```bash
issuectl --json next
issuectl --json next --limit 5
issuectl --json next --wait
```

Behavior notes:

- `next` returns currently actionable items plus explanation text for why they are actionable.
- `next --wait` blocks until at least one actionable item exists.
- Empty `next` returns exit code `3`.

Use `review tree` to understand review state and what blocks progress.

```bash
issuectl --json review tree
issuectl review tree APP-12
```

Review states:

- `REVIEW`: the item itself is not ready.
- `WAIT`: a descendant is not ready.
- `OPEN`: descendants are still open.
- `CLEAR`: the item and descendants are in a clear state.

## History and undo

Use history to understand how an item changed and `undo` to reverse a command when allowed.

```bash
issuectl --json history list
issuectl --json history show APP-12
issuectl --json history command CMD-18
issuectl --json undo CMD-18
```

Important:

- Undo operates on a prior command ID.
- Undo may be rejected if later changes exist for the same entity.
- Project updates such as prefix changes can be undone.
- Project creation still cannot be undone.

## Recommended operating patterns

### Triage a project

```bash
issuectl --json project show --explain
issuectl --json next --limit 10
issuectl --json item list --status todo --ready false
issuectl --json item list --blocked true
issuectl --json review tree
```

### Inspect a branch of work

```bash
issuectl --json item show APP-12
issuectl --json item tree APP-12
issuectl --json item children APP-12
issuectl --json item blockers APP-12
issuectl --json history show APP-12
```

### Create and prepare follow-up work

```bash
issuectl --json item create --title "Investigate flaky tests" --priority high
issuectl --json item block APP-20 --by APP-21
issuectl --json item ready APP-21
```

## Decision guidance

- Use `item update` when changing several fields at once.
- Use `item status`, `item ready`, and `item move` when making one targeted change.
- Use `item tree` for hierarchy, `item children` for immediate descendants, and `history show` for audit trail.
- Use `project show --explain` when project targeting is unclear.
- Use `--project` on writes when the target project should not depend on current repo context or global override.
- Use `--json` by default in agent workflows.

## Failure handling

- If `project show` or item commands fail because there is no active project, use `issuectl init` in the repo or `issuectl project list` and `issuectl project use <PROJECT_ID>`.
- If `next` exits with code `3`, treat that as "nothing actionable yet."
- If `undo` is rejected, inspect `history command` and item history instead of trying to force it.
- If a command fails validation, prefer another CLI command that expresses the intended state transition rather than bypassing the CLI.

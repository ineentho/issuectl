# issuecli

Local-first work item tracking for repository workflows.

## CLI

Run the existing CLI with subcommands like:

```bash
issuecli init
issuecli item create --title "Task"
issuecli next
```

## UI

Launch the native GPUI app with:

```bash
cargo run -- ui
```

The UI shares the same SQLite database and current project resolution as the CLI. It provides:

- project switching
- item overview and hierarchy
- next queue
- item detail and recent history
- common mutations such as create, edit, move, ready/unready, status changes, blockers, and undo

## Notes

- On macOS, GPUI depends on the normal Xcode / Metal toolchain prerequisites.
- The default database path remains `~/.issuecli/db.sqlite3` unless `ISSUECLI_DB_PATH` is set.

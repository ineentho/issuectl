# issuectl

Local-first work item tracking for repository workflows.

## CLI

Run the existing CLI with subcommands like:

```bash
issuectl init
issuectl item create --title "Task"
issuectl next
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
- The default database path remains `~/.issuectl/db.sqlite3` unless `ISSUECTL_DB_PATH` is set.

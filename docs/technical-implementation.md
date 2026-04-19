# Technical Implementation

## Purpose

This document translates the product outline into concrete implementation decisions for the MVP.

It is intended to guide the initial Rust implementation of issuecli and keep architectural choices explicit as the codebase is created.

## MVP Technical Direction

- Language: Rust
- CLI library: `clap`
- Storage: SQLite
- Database location: `~/.issuecli/db.sqlite3`
- Output modes: human-readable by default, `--json` for machine-readable output
- Architecture style: single local CLI process per invocation, coordinated through the database

## Rust Runtime Choice

Rust is a good fit for this project because it supports:

- A single compiled binary for local use
- Strong correctness guarantees for stateful CLI behavior
- Good SQLite support
- Predictable performance for frequent local invocations
- Straightforward distribution without requiring a runtime dependency

The implementation should optimize for clarity and operational reliability rather than language-level cleverness.

## CLI Parsing

The CLI should use `clap` as the command-line parsing library.

Reasons:

- Mature and widely adopted
- Strong support for nested subcommands
- Good derive-based ergonomics
- Built-in validation and help text generation
- A natural fit for the command structure already defined in the project outline

Recommended approach:

- Use `clap` derive macros
- Keep argument parsing types separate from execution logic
- Represent constrained values such as status and priority with enums where appropriate

Likely command shape:

- `init`
- `project ...`
- `item ...`
- `next`
- `history ...`
- `undo ...`

## Suggested Crates

The initial implementation should stay small and conservative.

Recommended starting dependencies:

- `clap` for CLI parsing
- `rusqlite` with the `bundled` feature for SQLite access
- `anyhow` for top-level error handling
- `thiserror` for structured domain errors
- `serde` for JSON serialization
- `serde_json` for JSON output
- `dirs` or `directories` for home-directory path resolution

Additional crates should only be added when they clearly reduce complexity.

## SQLite Access Layer

The implementation should use `rusqlite` with the `bundled` feature enabled and prefer raw SQL over a heavy ORM or abstraction layer.

Reasons:

- The schema is small and well understood
- SQL behavior is part of the product design
- Raw SQL keeps migrations, locking behavior, and transactions explicit
- This project benefits more from predictability than from ORM convenience
- Bundling SQLite reduces local environment variability during development and installation

Recommended approach:

- Use a thin repository or storage layer around `rusqlite`
- Keep SQL statements close to the code that owns the workflow
- Avoid over-abstracting queries early

## Schema Management

Schema creation and migration should be explicit and versioned.

Recommended MVP approach:

- Store ordered SQL migrations in the repository
- Embed them into the binary at compile time
- Apply them automatically during startup before command execution
- Track applied migrations in the database

The migration system does not need to be sophisticated at first, but it must be deterministic and safe.

Recommended structure:

- A `migrations/` directory in the repository
- Numbered migration files such as `0001_init.sql`, `0002_add_blockers.sql`
- A small migration runner in the application
- A `schema_migrations` table recording applied versions and timestamps

Recommended runtime behavior:

1. Open the database connection.
2. Ensure the `schema_migrations` table exists.
3. Read the embedded migration list in order.
4. Apply any unapplied migrations inside transactions.
5. Record each successful migration in `schema_migrations`.
6. Continue startup only after the database is up to date.

Why this is a good fit:

- SQL remains explicit and easy to review
- The binary is self-contained and does not depend on external migration files at runtime
- Startup behavior is deterministic across environments
- `rusqlite` works well with a small hand-rolled migration runner

The MVP should avoid a heavy migration framework unless the migration surface becomes substantially more complex.

## Database Initialization

The implementation should create and manage a single global database at:

- `~/.issuecli/db.sqlite3`

The startup path should ensure:

- Parent directories exist
- The database file is created if missing
- Required schema migrations are applied
- SQLite pragmas such as WAL mode are configured

## SQLite Configuration

The implementation should treat SQLite as both the persistence layer and the low-level concurrency primitive.

Recommended initial configuration:

- WAL mode enabled
- Foreign keys enabled
- Busy timeout configured
- Short write transactions only

The application-level lease remains responsible for higher-level write coordination.

## Identifier Strategy

The CLI should use stable, short uppercase identifiers.

MVP formats:

- Work items: `WI-12`
- Commands: `CMD-104`

Allocation strategy:

- Work-item numeric ids should be per-project sequences
- Command numeric ids may be globally sequenced within the single database

The implementation should generate identifiers transactionally so concurrent writers cannot allocate duplicate ids.

## Project Resolution

Project resolution should be repository-aware and deterministic.

Recommended behavior:

- Resolve the current Git worktree root
- Normalize the root path through symlink resolution
- Use the resolved absolute path as the canonical project anchor
- Look up or create the associated project in the global database

If the current working directory is not inside a Git repository, commands that require a project should fail clearly.

## Application Structure

The initial codebase should be organized around a small number of clear modules.

Suggested structure:

- `cli`: argument definitions and command dispatch
- `commands`: command handlers
- `db`: connection setup, migrations, and low-level storage helpers
- `model`: domain types such as work items, projects, and history records
- `output`: human-readable and JSON rendering
- `git`: repository root detection and path resolution
- `locking`: application lease acquisition and release

This structure should remain lightweight. The goal is separation of responsibilities, not framework-style layering.

## Output Strategy

The CLI should support two output modes:

- Human-readable output by default
- `--json` for machine-readable output

Implementation guidance:

- Define stable serializable response types for JSON output
- Avoid mixing prose into JSON mode
- Keep human output concise and readable
- Ensure error responses also have a stable JSON shape when `--json` is enabled

## Error Handling

The implementation should distinguish between:

- usage errors
- validation errors
- operational errors
- empty-result conditions

Recommended behavior:

- Use typed domain errors internally
- Convert them to stable exit codes at the top level
- Preserve concise human-readable messages
- Return structured JSON errors in JSON mode

## Exit Codes

Recommended MVP mapping:

- `0` success
- `1` general operational or validation error
- `2` usage error
- `3` expected empty result, such as `issuecli next` finding no unblocked work items

## Transaction Model

Every mutating command should execute as a single transaction.

General pattern:

1. Acquire the application lease.
2. Open a write transaction.
3. Validate command preconditions.
4. Allocate identifiers if needed.
5. Insert command history.
6. Insert entity history events.
7. Update current-state tables.
8. Commit.

If any step fails, the entire command must roll back.

## Locking Model

The implementation should combine SQLite locking with an explicit application-level lease.

Recommended responsibilities:

- SQLite handles low-level transactional correctness
- The lease provides clear single-writer semantics
- The lease records ownership and expiry
- Stale leases can be detected and recovered from safely

This keeps the concurrency model understandable for both humans and LLMs.

## Testing Strategy

The MVP should emphasize integration tests over elaborate mock-heavy unit tests.

Automated tests should be treated as a first-class part of the implementation rather than a later hardening step.

Recommended automated test layers:

- Unit tests for small pure functions and validation helpers
- Integration tests against a real temporary SQLite database
- CLI-level tests that execute commands end-to-end and assert on output, exit codes, and side effects

The default approach should be to test the real system behavior whenever practical.

Recommended coverage:

- Schema initialization
- Automatic migration application on startup
- Project resolution from Git repositories
- Symlink-normalized repository resolution
- Work-item creation and listing
- Work-item status transitions and `closed_at` behavior
- `ready` workflow behavior
- Parent-child validation
- Blocking-cycle validation
- `next` selection semantics
- Undo refusal when a later command changed the same entity
- Concurrent writer behavior against a real SQLite database
- JSON output shape for key commands
- Exit-code behavior, including no-result behavior for `issuecli next`

Unit tests are still useful for smaller pure functions, but real SQLite-backed tests should be the default for core behavior.

### Integration Test Environment

Integration tests should create isolated temporary environments rather than relying on the developer's real home directory or an existing repository.

Recommended test setup:

- Use temporary directories for filesystem state
- Use a temporary database path instead of the production home-directory path
- Create temporary Git repositories inside tests when project resolution is being exercised
- Keep tests independent so they can run in parallel when safe

Tests that exercise locking and concurrency may need to run serially if they share assumptions about timing.

### CLI-Level Testing

CLI-level tests should execute the compiled binary and assert on:

- stdout
- stderr
- exit codes
- resulting database state when relevant

This is especially important because the product contract is largely expressed through command behavior and machine-readable output.

### Automation Workflow

The repository should have a simple default automated test workflow.

Recommended commands:

- `cargo test` for the standard automated test suite
- `cargo fmt --check` for formatting validation
- `cargo clippy -- -D warnings` for lint enforcement

The implementation should aim for these commands to run cleanly in local development and CI.

### Test Design Principles

- Prefer deterministic tests over timing-sensitive tests
- Avoid mocks for SQLite, Git resolution, and command execution where real behavior is feasible
- Keep fixtures small and explicit
- Assert on externally visible behavior, not only internal implementation details
- Add regression tests for every bug fixed in core workflow logic

## Initial Implementation Order

Recommended build order:

1. Set up the Rust crate and CLI skeleton with `clap`
2. Implement database path resolution and initialization
3. Add migrations and base schema
4. Implement project resolution from Git worktree roots
5. Implement `issuecli init`
6. Implement core work-item read and write commands
7. Add event history recording
8. Add `next`
9. Add undo and refusal rules
10. Add JSON output stabilization and exit-code coverage

## Non-Goals For The Initial Implementation

- No plugin system
- No networked service layer
- No daemon process
- No ORM
- No interactive TUI
- No attempt to optimize every query before usage patterns are understood

The first implementation should be direct, testable, and easy to evolve.

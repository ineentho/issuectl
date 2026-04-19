CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS locks (
    lock_name TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    leased_until TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    public_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    repo_root TEXT UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    next_item_number INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS project_overrides (
    scope TEXT PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id)
);

CREATE TABLE IF NOT EXISTS commands (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    public_id TEXT NOT NULL UNIQUE,
    project_id INTEGER REFERENCES projects(id),
    action TEXT NOT NULL,
    actor TEXT NOT NULL,
    undone_command_id INTEGER REFERENCES commands(id),
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS work_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    public_id TEXT NOT NULL,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    ready INTEGER NOT NULL,
    status TEXT NOT NULL,
    priority TEXT NOT NULL,
    parent_id INTEGER REFERENCES work_items(id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT,
    version INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_work_items_project_public_id ON work_items(project_id, public_id);

CREATE TABLE IF NOT EXISTS work_item_blockers (
    blocker_id INTEGER NOT NULL REFERENCES work_items(id),
    blocked_id INTEGER NOT NULL REFERENCES work_items(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY (blocker_id, blocked_id)
);

CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id INTEGER NOT NULL REFERENCES commands(id),
    project_id INTEGER REFERENCES projects(id),
    entity_type TEXT NOT NULL,
    entity_key TEXT NOT NULL,
    operation TEXT NOT NULL,
    before_state TEXT,
    after_state TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_projects_repo_root ON projects(repo_root);
CREATE INDEX IF NOT EXISTS idx_work_items_project ON work_items(project_id);
CREATE INDEX IF NOT EXISTS idx_work_items_parent ON work_items(parent_id);
CREATE INDEX IF NOT EXISTS idx_commands_project_created ON commands(project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_entity_created ON events(entity_type, entity_key, id);
CREATE INDEX IF NOT EXISTS idx_events_command ON events(command_id);

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sources (
    id TEXT PRIMARY KEY,
    data_json TEXT NOT NULL,
    secret_ref TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    data_json TEXT NOT NULL,
    secret_ref TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS gateway_keys (
    id TEXT PRIMARY KEY,
    data_json TEXT NOT NULL,
    secret_ref TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS pending_imports (
    id TEXT PRIMARY KEY,
    preview_json TEXT NOT NULL,
    secret_ref TEXT NOT NULL UNIQUE,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS wake_tasks (
    id TEXT PRIMARY KEY,
    data_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS wake_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    data_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS usage_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    local_key_id TEXT NOT NULL,
    candidate_kind TEXT NOT NULL,
    candidate_hint TEXT NOT NULL,
    requested_model TEXT,
    resolved_model TEXT,
    wire_api TEXT NOT NULL,
    success INTEGER NOT NULL,
    http_status INTEGER NOT NULL,
    error_category TEXT,
    latency_ms INTEGER NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_usage_created_at ON usage_events(created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_events(resolved_model);
CREATE INDEX IF NOT EXISTS idx_usage_success ON usage_events(success, created_at_ms DESC);

INSERT OR IGNORE INTO metadata(key, value) VALUES ('schema_version', '1');
INSERT OR IGNORE INTO metadata(key, value) VALUES ('gateway_enabled', 'true');

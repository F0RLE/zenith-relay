pub(in crate::local_pool::store::telemetry_db) const MIGRATION_001: &str = r#"
CREATE TABLE IF NOT EXISTS request_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    request_id TEXT NOT NULL,
    local_key_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    requested_model TEXT,
    resolved_model TEXT,
    wire_api TEXT NOT NULL,
    success INTEGER NOT NULL,
    http_status INTEGER NOT NULL,
    error_category TEXT,
    latency_ms INTEGER NOT NULL,
    ttft_ms INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS request_logs_request_id_idx ON request_logs(request_id);
CREATE INDEX IF NOT EXISTS request_logs_created_at_idx ON request_logs(created_at);
CREATE INDEX IF NOT EXISTS request_logs_source_created_idx ON request_logs(source_id, created_at);
CREATE INDEX IF NOT EXISTS request_logs_key_created_idx ON request_logs(local_key_id, created_at);
PRAGMA user_version = 1;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_002: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
DROP INDEX IF EXISTS request_logs_request_id_idx;
CREATE UNIQUE INDEX request_logs_request_attempt_idx ON request_logs(request_id, attempt);
PRAGMA user_version = 2;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_003: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN candidate_id TEXT;
ALTER TABLE request_logs ADD COLUMN account_id TEXT;
CREATE INDEX request_logs_candidate_created_idx ON request_logs(candidate_id, created_at);
CREATE INDEX request_logs_account_created_idx ON request_logs(account_id, created_at);
PRAGMA user_version = 3;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_004: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cached_input_tokens INTEGER;
PRAGMA user_version = 4;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_005: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN reasoning_tokens INTEGER;
PRAGMA user_version = 5;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_006: &str = r#"
BEGIN IMMEDIATE;
CREATE TRIGGER request_logs_retention
AFTER INSERT ON request_logs
WHEN NEW.id % 256 = 0
BEGIN
    DELETE FROM request_logs WHERE created_at < datetime('now', '-30 days');
END;
PRAGMA user_version = 6;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_007: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN routing_json TEXT;
PRAGMA user_version = 7;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_008: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cache_write_input_tokens INTEGER;
PRAGMA user_version = 8;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_009: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE response_affinity (
    response_key TEXT PRIMARY KEY,
    candidate_id TEXT NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX response_affinity_expires_idx ON response_affinity(expires_at_ms);
PRAGMA user_version = 9;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_010: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN generation_ms INTEGER;
PRAGMA user_version = 10;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_011: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs DROP COLUMN cache_write_input_tokens;
PRAGMA user_version = 11;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_012: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cache_write_input_tokens INTEGER;
PRAGMA user_version = 12;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_013: &str = r#"
BEGIN IMMEDIATE;
CREATE INDEX response_affinity_updated_idx
    ON response_affinity(updated_at_ms DESC, response_key DESC);
DELETE FROM response_affinity
WHERE response_key IN (
    SELECT response_key FROM response_affinity
    ORDER BY updated_at_ms DESC, response_key DESC
    LIMIT -1 OFFSET 4096
);
CREATE TRIGGER response_affinity_retention
AFTER INSERT ON response_affinity
BEGIN
    DELETE FROM response_affinity WHERE expires_at_ms <= NEW.updated_at_ms;
    DELETE FROM response_affinity
    WHERE response_key IN (
        SELECT response_key FROM response_affinity
        ORDER BY updated_at_ms DESC, response_key DESC
        LIMIT -1 OFFSET 4096
    );
END;
PRAGMA user_version = 13;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_014: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE app_state (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);
PRAGMA user_version = 14;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_015: &str = r#"
BEGIN IMMEDIATE;
DROP INDEX IF EXISTS request_logs_request_attempt_idx;
DELETE FROM request_logs
WHERE id NOT IN (
    SELECT MAX(id) FROM request_logs GROUP BY request_id
);
CREATE UNIQUE INDEX request_logs_request_id_idx ON request_logs(request_id);
PRAGMA user_version = 15;
COMMIT;
"#;

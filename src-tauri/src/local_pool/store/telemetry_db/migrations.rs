pub(super) const MIGRATION_001: &str = r#"
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

pub(super) const MIGRATION_002: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
DROP INDEX IF EXISTS request_logs_request_id_idx;
CREATE UNIQUE INDEX request_logs_request_attempt_idx ON request_logs(request_id, attempt);
PRAGMA user_version = 2;
COMMIT;
"#;

pub(super) const MIGRATION_003: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN candidate_id TEXT;
ALTER TABLE request_logs ADD COLUMN account_id TEXT;
CREATE INDEX request_logs_candidate_created_idx ON request_logs(candidate_id, created_at);
CREATE INDEX request_logs_account_created_idx ON request_logs(account_id, created_at);
PRAGMA user_version = 3;
COMMIT;
"#;

pub(super) const MIGRATION_004: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cached_input_tokens INTEGER;
PRAGMA user_version = 4;
COMMIT;
"#;

pub(super) const MIGRATION_005: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN reasoning_tokens INTEGER;
PRAGMA user_version = 5;
COMMIT;
"#;

pub(super) const MIGRATION_006: &str = r#"
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

pub(super) const MIGRATION_007: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN routing_json TEXT;
PRAGMA user_version = 7;
COMMIT;
"#;

pub(super) const MIGRATION_008: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cache_write_input_tokens INTEGER;
PRAGMA user_version = 8;
COMMIT;
"#;

pub(super) const MIGRATION_009: &str = r#"
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

pub(super) const MIGRATION_010: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN generation_ms INTEGER;
PRAGMA user_version = 10;
COMMIT;
"#;

pub(super) const MIGRATION_011: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs DROP COLUMN cache_write_input_tokens;
PRAGMA user_version = 11;
COMMIT;
"#;

pub(super) const MIGRATION_012: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cache_write_input_tokens INTEGER;
PRAGMA user_version = 12;
COMMIT;
"#;

pub(super) const MIGRATION_013: &str = r#"
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

pub(super) const MIGRATION_014: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE app_state (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);
PRAGMA user_version = 14;
COMMIT;
"#;

pub(super) const MIGRATION_015: &str = r#"
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

pub(super) const MIGRATION_016: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN service_tier TEXT NOT NULL DEFAULT 'standard';
ALTER TABLE request_logs ADD COLUMN effective_credits_milli INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version = 16;
COMMIT;
"#;

pub(super) const MIGRATION_017: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs DROP COLUMN effective_credits_milli;
PRAGMA user_version = 17;
COMMIT;
"#;

pub(super) const MIGRATION_018: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN applied_service_tier TEXT;
PRAGMA user_version = 18;
COMMIT;
"#;

pub(super) const MIGRATION_019: &str = r#"
BEGIN IMMEDIATE;
DROP TRIGGER IF EXISTS request_logs_retention;
CREATE TABLE usage_candidate_rollups (
    candidate_kind TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    input_samples INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_samples INTEGER NOT NULL DEFAULT 0,
    cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_input_samples INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    output_samples INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_samples INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(candidate_kind, candidate_id, model)
) WITHOUT ROWID;
PRAGMA user_version = 19;
COMMIT;
"#;

pub(super) const MIGRATION_020: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE performance_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    name TEXT NOT NULL,
    duration_ms REAL NOT NULL,
    context TEXT
);
CREATE INDEX performance_samples_created_idx ON performance_samples(created_at DESC);
CREATE TRIGGER performance_samples_retention
AFTER INSERT ON performance_samples
BEGIN
    DELETE FROM performance_samples WHERE created_at < datetime('now', '-30 days');
    DELETE FROM performance_samples WHERE id <= NEW.id - 2048;
END;
PRAGMA user_version = 20;
COMMIT;
"#;

pub(super) const MIGRATION_021: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN tool_use_json TEXT;
PRAGMA user_version = 21;
COMMIT;
"#;

pub(super) const MIGRATION_022: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN error_origin TEXT;
PRAGMA user_version = 22;
COMMIT;
"#;

pub(super) const MIGRATION_023: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN requested_reasoning_effort TEXT;
ALTER TABLE request_logs ADD COLUMN effective_reasoning_effort TEXT;
PRAGMA user_version = 23;
COMMIT;
"#;

pub(super) const LOCAL_DATABASE_SCHEMA_VERSION: u32 = 23;
pub(super) const MAX_RESPONSE_AFFINITY_ROWS: usize = 4_096;
pub(super) const MAX_STATE_JSON_BYTES: usize = 16 * 1024 * 1024;
pub(super) const ARCHIVE_USAGE_SQL: &str = r#"
INSERT INTO usage_candidate_rollups(
    candidate_kind, candidate_id, model,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples, output_tokens, output_samples,
    total_tokens, total_samples
)
SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
    COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
    COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
    COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
    COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
    COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
FROM request_logs
WHERE created_at < datetime('now', '-30 days')
GROUP BY 1, 2, 3
ON CONFLICT(candidate_kind, candidate_id, model) DO UPDATE SET
    input_tokens=input_tokens + excluded.input_tokens,
    input_samples=input_samples + excluded.input_samples,
    cached_input_tokens=cached_input_tokens + excluded.cached_input_tokens,
    cached_input_samples=cached_input_samples + excluded.cached_input_samples,
    cache_write_input_tokens=cache_write_input_tokens + excluded.cache_write_input_tokens,
    cache_write_input_samples=cache_write_input_samples + excluded.cache_write_input_samples,
    output_tokens=output_tokens + excluded.output_tokens,
    output_samples=output_samples + excluded.output_samples,
    total_tokens=total_tokens + excluded.total_tokens,
    total_samples=total_samples + excluded.total_samples;
DELETE FROM request_logs WHERE created_at < datetime('now', '-30 days');
"#;

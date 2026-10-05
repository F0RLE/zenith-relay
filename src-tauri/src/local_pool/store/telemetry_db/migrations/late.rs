pub(in crate::local_pool::store::telemetry_db) const MIGRATION_016: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN service_tier TEXT NOT NULL DEFAULT 'standard';
ALTER TABLE request_logs ADD COLUMN effective_credits_milli INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version = 16;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_017: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs DROP COLUMN effective_credits_milli;
PRAGMA user_version = 17;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_018: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN applied_service_tier TEXT;
PRAGMA user_version = 18;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_019: &str = r#"
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

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_020: &str = r#"
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

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_021: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN tool_use_json TEXT;
PRAGMA user_version = 21;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_022: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN error_origin TEXT;
PRAGMA user_version = 22;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_023: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN requested_reasoning_effort TEXT;
ALTER TABLE request_logs ADD COLUMN effective_reasoning_effort TEXT;
PRAGMA user_version = 23;
COMMIT;
"#;

// Account records live as one JSON array in `app_state`. Move the only
// user-authored value that used to live inside quota economics before removing
// the legacy object. A direct value always wins so a previously saved edit is
// never overwritten by an older nested value.
pub(in crate::local_pool::store::telemetry_db) const MIGRATION_024: &str = r#"
BEGIN IMMEDIATE;
UPDATE app_state
SET value_json = COALESCE((
    SELECT json_group_array(json(
        CASE
            WHEN (
                json_type(value, '$.purchaseCostMicroUsd') IS NULL
                OR json_type(value, '$.purchaseCostMicroUsd') = 'null'
            )
                AND json_type(value, '$.economics.purchaseCostMicroUsd') IS NOT NULL
            THEN json_remove(
                json_set(
                    value,
                    '$.purchaseCostMicroUsd',
                    json_extract(value, '$.economics.purchaseCostMicroUsd')
                ),
                '$.economics'
            )
            ELSE json_remove(value, '$.economics')
        END
    ) ORDER BY CAST(key AS INTEGER))
    FROM json_each(app_state.value_json)
), '[]')
WHERE key = 'accounts'
    AND json_valid(value_json)
    AND json_type(value_json) = 'array'
    AND EXISTS (
        SELECT 1
        FROM json_each(app_state.value_json)
        WHERE json_type(value, '$.economics') IS NOT NULL
    );
PRAGMA user_version = 24;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_025: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cache_write_ttl TEXT;
PRAGMA user_version = 25;
COMMIT;
"#;

// Keep the API-equivalent aggregate independent from the short-lived request
// log table. New records update it transactionally; old databases are rebuilt
// once here before raw logs older than the retention window are removed.
pub(in crate::local_pool::store::telemetry_db) const MIGRATION_026: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs
    ADD COLUMN usage_aggregate_recorded INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_candidate_rollups
    ADD COLUMN cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_candidate_rollups
    ADD COLUMN cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_candidate_rollups
    ADD COLUMN unknown_cache_write_tokens INTEGER NOT NULL DEFAULT 0;
UPDATE usage_candidate_rollups
SET unknown_cache_write_tokens = cache_write_input_tokens;
INSERT INTO usage_candidate_rollups(
    candidate_kind, candidate_id, model,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples,
    cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
    output_tokens, output_samples, total_tokens, total_samples
)
SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
    COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
    COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
    COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h')
        THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
    COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
FROM request_logs
GROUP BY 1, 2, 3
ON CONFLICT(candidate_kind, candidate_id, model) DO UPDATE SET
    input_tokens = input_tokens + excluded.input_tokens,
    input_samples = input_samples + excluded.input_samples,
    cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
    cached_input_samples = cached_input_samples + excluded.cached_input_samples,
    cache_write_input_tokens = cache_write_input_tokens + excluded.cache_write_input_tokens,
    cache_write_input_samples = cache_write_input_samples + excluded.cache_write_input_samples,
    cache_write_5m_tokens = cache_write_5m_tokens + excluded.cache_write_5m_tokens,
    cache_write_1h_tokens = cache_write_1h_tokens + excluded.cache_write_1h_tokens,
    unknown_cache_write_tokens = unknown_cache_write_tokens + excluded.unknown_cache_write_tokens,
    output_tokens = output_tokens + excluded.output_tokens,
    output_samples = output_samples + excluded.output_samples,
    total_tokens = total_tokens + excluded.total_tokens,
    total_samples = total_samples + excluded.total_samples;
UPDATE request_logs SET usage_aggregate_recorded = 1;
PRAGMA user_version = 26;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_027: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN client_context_id TEXT;
PRAGMA user_version = 27;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_028: &str = r#"
BEGIN IMMEDIATE;
DROP TRIGGER IF EXISTS response_affinity_retention;
DELETE FROM response_affinity
WHERE response_key IN (
    SELECT response_key FROM response_affinity
    ORDER BY updated_at_ms DESC, response_key DESC
    LIMIT -1 OFFSET 16384
);
CREATE TRIGGER response_affinity_retention
AFTER INSERT ON response_affinity
BEGIN
    DELETE FROM response_affinity WHERE expires_at_ms <= NEW.updated_at_ms;
    DELETE FROM response_affinity
    WHERE response_key IN (
        SELECT response_key FROM response_affinity
        ORDER BY updated_at_ms DESC, response_key DESC
        LIMIT -1 OFFSET 16384
    );
END;
PRAGMA user_version = 28;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_029: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN upstream_error_json TEXT;
PRAGMA user_version = 29;
COMMIT;
"#;

// Split retained usage by the observed service tier and the prompt-size band.
// Rows still in the request log are rebuilt exactly. Tokens whose raw log was
// already deleted stay on the standard/base schedule, because the old rollup
// no longer knows which request they came from.
pub(in crate::local_pool::store::telemetry_db) const MIGRATION_030: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE usage_candidate_rollups_v2 (
    candidate_kind TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    model TEXT NOT NULL,
    price_class TEXT NOT NULL DEFAULT 'standard',
    context_band TEXT NOT NULL DEFAULT 'base',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    input_samples INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_samples INTEGER NOT NULL DEFAULT 0,
    cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_input_samples INTEGER NOT NULL DEFAULT 0,
    cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0,
    unknown_cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    output_samples INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_samples INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(candidate_kind, candidate_id, model, price_class, context_band)
) WITHOUT ROWID;
INSERT INTO usage_candidate_rollups_v2(
    candidate_kind, candidate_id, model, price_class, context_band,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples,
    cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
    output_tokens, output_samples, total_tokens, total_samples
)
SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
    CASE lower(COALESCE(applied_service_tier, ''))
        WHEN 'flex' THEN 'flex'
        WHEN 'priority' THEN 'priority'
        WHEN 'fast' THEN 'priority'
        ELSE 'standard' END,
    CASE
        WHEN COALESCE(input_tokens, 0) > 272000 THEN 'above_272k'
        WHEN COALESCE(input_tokens, 0) > 200000 THEN 'above_200k'
        ELSE 'base' END,
    COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
    COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
    COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h')
        THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
    COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
FROM request_logs
GROUP BY 1, 2, 3, 4, 5;
INSERT INTO usage_candidate_rollups_v2(
    candidate_kind, candidate_id, model, price_class, context_band,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples,
    cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
    output_tokens, output_samples, total_tokens, total_samples
)
SELECT old.candidate_kind, old.candidate_id, old.model, 'standard', 'base',
    MAX(old.input_tokens - COALESCE(fresh.input_tokens, 0), 0),
    MAX(old.input_samples - COALESCE(fresh.input_samples, 0), 0),
    MAX(old.cached_input_tokens - COALESCE(fresh.cached_input_tokens, 0), 0),
    MAX(old.cached_input_samples - COALESCE(fresh.cached_input_samples, 0), 0),
    MAX(old.cache_write_input_tokens - COALESCE(fresh.cache_write_input_tokens, 0), 0),
    MAX(old.cache_write_input_samples - COALESCE(fresh.cache_write_input_samples, 0), 0),
    MAX(old.cache_write_5m_tokens - COALESCE(fresh.cache_write_5m_tokens, 0), 0),
    MAX(old.cache_write_1h_tokens - COALESCE(fresh.cache_write_1h_tokens, 0), 0),
    MAX(old.unknown_cache_write_tokens - COALESCE(fresh.unknown_cache_write_tokens, 0), 0),
    MAX(old.output_tokens - COALESCE(fresh.output_tokens, 0), 0),
    MAX(old.output_samples - COALESCE(fresh.output_samples, 0), 0),
    MAX(old.total_tokens - COALESCE(fresh.total_tokens, 0), 0),
    MAX(old.total_samples - COALESCE(fresh.total_samples, 0), 0)
FROM usage_candidate_rollups AS old
LEFT JOIN (
    SELECT candidate_kind, candidate_id, model,
        SUM(input_tokens) AS input_tokens,
        SUM(input_samples) AS input_samples,
        SUM(cached_input_tokens) AS cached_input_tokens,
        SUM(cached_input_samples) AS cached_input_samples,
        SUM(cache_write_input_tokens) AS cache_write_input_tokens,
        SUM(cache_write_input_samples) AS cache_write_input_samples,
        SUM(cache_write_5m_tokens) AS cache_write_5m_tokens,
        SUM(cache_write_1h_tokens) AS cache_write_1h_tokens,
        SUM(unknown_cache_write_tokens) AS unknown_cache_write_tokens,
        SUM(output_tokens) AS output_tokens,
        SUM(output_samples) AS output_samples,
        SUM(total_tokens) AS total_tokens,
        SUM(total_samples) AS total_samples
    FROM usage_candidate_rollups_v2
    GROUP BY candidate_kind, candidate_id, model
) AS fresh
    ON fresh.candidate_kind = old.candidate_kind
    AND fresh.candidate_id = old.candidate_id
    AND fresh.model = old.model
WHERE old.input_tokens > COALESCE(fresh.input_tokens, 0)
    OR old.output_tokens > COALESCE(fresh.output_tokens, 0)
    OR old.total_tokens > COALESCE(fresh.total_tokens, 0)
    OR old.cached_input_tokens > COALESCE(fresh.cached_input_tokens, 0)
    OR old.cache_write_input_tokens > COALESCE(fresh.cache_write_input_tokens, 0)
ON CONFLICT(candidate_kind, candidate_id, model, price_class, context_band) DO UPDATE SET
    input_tokens = input_tokens + excluded.input_tokens,
    input_samples = input_samples + excluded.input_samples,
    cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
    cached_input_samples = cached_input_samples + excluded.cached_input_samples,
    cache_write_input_tokens = cache_write_input_tokens + excluded.cache_write_input_tokens,
    cache_write_input_samples = cache_write_input_samples + excluded.cache_write_input_samples,
    cache_write_5m_tokens = cache_write_5m_tokens + excluded.cache_write_5m_tokens,
    cache_write_1h_tokens = cache_write_1h_tokens + excluded.cache_write_1h_tokens,
    unknown_cache_write_tokens = unknown_cache_write_tokens + excluded.unknown_cache_write_tokens,
    output_tokens = output_tokens + excluded.output_tokens,
    output_samples = output_samples + excluded.output_samples,
    total_tokens = total_tokens + excluded.total_tokens,
    total_samples = total_samples + excluded.total_samples;
UPDATE request_logs SET usage_aggregate_recorded = 1;
DROP TABLE usage_candidate_rollups;
ALTER TABLE usage_candidate_rollups_v2 RENAME TO usage_candidate_rollups;
PRAGMA user_version = 30;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_031: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN transport TEXT NOT NULL DEFAULT 'http';
PRAGMA user_version = 31;
COMMIT;
"#;

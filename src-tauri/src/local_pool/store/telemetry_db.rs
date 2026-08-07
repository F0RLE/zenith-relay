use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension,
    TransactionBehavior,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Instant,
};
use zenith_relay_core::{
    api_pricing_revision, estimate_api_equivalent_with_price_override,
    protocol::{UsageBucket, UsageGroup, UsageQuery, UsageTotals},
    ApiEquivalentSummary, ApiModelPriceOverride, DefaultServiceTier, ResponseAffinityBinding,
    RoutingDiagnostics, ToolUseDiagnostics, UsageEvent, WireApi,
};

pub type SourcePriceOverrides = BTreeMap<String, BTreeMap<String, ApiModelPriceOverride>>;

const MIGRATION_001: &str = r#"
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

const MIGRATION_002: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
DROP INDEX IF EXISTS request_logs_request_id_idx;
CREATE UNIQUE INDEX request_logs_request_attempt_idx ON request_logs(request_id, attempt);
PRAGMA user_version = 2;
COMMIT;
"#;

const MIGRATION_003: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN candidate_id TEXT;
ALTER TABLE request_logs ADD COLUMN account_id TEXT;
CREATE INDEX request_logs_candidate_created_idx ON request_logs(candidate_id, created_at);
CREATE INDEX request_logs_account_created_idx ON request_logs(account_id, created_at);
PRAGMA user_version = 3;
COMMIT;
"#;

const MIGRATION_004: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cached_input_tokens INTEGER;
PRAGMA user_version = 4;
COMMIT;
"#;

const MIGRATION_005: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN reasoning_tokens INTEGER;
PRAGMA user_version = 5;
COMMIT;
"#;

const MIGRATION_006: &str = r#"
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

const MIGRATION_007: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN routing_json TEXT;
PRAGMA user_version = 7;
COMMIT;
"#;

const MIGRATION_008: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cache_write_input_tokens INTEGER;
PRAGMA user_version = 8;
COMMIT;
"#;

const MIGRATION_009: &str = r#"
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

const MIGRATION_010: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN generation_ms INTEGER;
PRAGMA user_version = 10;
COMMIT;
"#;

const MIGRATION_011: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs DROP COLUMN cache_write_input_tokens;
PRAGMA user_version = 11;
COMMIT;
"#;

const MIGRATION_012: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN cache_write_input_tokens INTEGER;
PRAGMA user_version = 12;
COMMIT;
"#;

const MIGRATION_013: &str = r#"
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

const MIGRATION_014: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE app_state (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);
PRAGMA user_version = 14;
COMMIT;
"#;

const MIGRATION_015: &str = r#"
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

const MIGRATION_016: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN service_tier TEXT NOT NULL DEFAULT 'standard';
ALTER TABLE request_logs ADD COLUMN effective_credits_milli INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version = 16;
COMMIT;
"#;

const MIGRATION_017: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs DROP COLUMN effective_credits_milli;
PRAGMA user_version = 17;
COMMIT;
"#;

const MIGRATION_018: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN applied_service_tier TEXT;
PRAGMA user_version = 18;
COMMIT;
"#;

const MIGRATION_019: &str = r#"
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

const MIGRATION_020: &str = r#"
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

const MIGRATION_021: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN tool_use_json TEXT;
PRAGMA user_version = 21;
COMMIT;
"#;

const LOCAL_DATABASE_SCHEMA_VERSION: u32 = 21;
const MAX_RESPONSE_AFFINITY_ROWS: usize = 4_096;
const MAX_STATE_JSON_BYTES: usize = 16 * 1024 * 1024;
const ARCHIVE_USAGE_SQL: &str = r#"
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

pub struct TelemetryDb {
    connection: Mutex<Connection>,
    usage_revision: AtomicU64,
    api_equivalent_cache: Mutex<Option<CachedUsageEquivalents>>,
    open_duration_ms: f64,
}

#[derive(Clone)]
struct CachedUsageEquivalents {
    usage_revision: u64,
    pricing_revision: String,
    value: UsageEquivalents,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLog {
    pub id: i64,
    pub created_at: String,
    pub request_id: String,
    pub attempt: u16,
    pub source_id: String,
    pub candidate_id: Option<String>,
    pub account_id: Option<String>,
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: String,
    pub service_tier: DefaultServiceTier,
    pub applied_service_tier: Option<DefaultServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub tool_use: Option<ToolUseDiagnostics>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub api_equivalent: ApiEquivalentSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsagePage {
    pub events: Vec<UsageLog>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    pub totals: UsageTotals,
    pub models: Vec<UsageGroup>,
    pub pool_members: Vec<UsageGroup>,
    pub buckets: Vec<UsageBucket>,
}

#[derive(Clone, Default)]
pub struct UsageEquivalents {
    pub accounts: HashMap<String, ApiEquivalentSummary>,
    pub sources: HashMap<String, ApiEquivalentSummary>,
}

impl TelemetryDb {
    pub fn open(path: &Path) -> Result<Self> {
        let started = Instant::now();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_error)?;
        if version > LOCAL_DATABASE_SCHEMA_VERSION {
            return Err(LocalPoolError::new(
                ErrorCode::UnsupportedSchema,
                format!(
                    "local database schema {version} is newer than supported schema {LOCAL_DATABASE_SCHEMA_VERSION}"
                ),
            ));
        }
        if version == 0 {
            connection.execute_batch(MIGRATION_001).map_err(db_error)?;
        }
        if version <= 1 {
            connection.execute_batch(MIGRATION_002).map_err(db_error)?;
        }
        if version <= 2 {
            connection.execute_batch(MIGRATION_003).map_err(db_error)?;
        }
        if version <= 3 {
            connection.execute_batch(MIGRATION_004).map_err(db_error)?;
        }
        if version <= 4 {
            connection.execute_batch(MIGRATION_005).map_err(db_error)?;
        }
        if version <= 5 {
            connection.execute_batch(MIGRATION_006).map_err(db_error)?;
        }
        if version <= 6 {
            connection.execute_batch(MIGRATION_007).map_err(db_error)?;
        }
        if version <= 7 {
            connection.execute_batch(MIGRATION_008).map_err(db_error)?;
        }
        if version <= 8 {
            connection.execute_batch(MIGRATION_009).map_err(db_error)?;
        }
        if version <= 9 {
            connection.execute_batch(MIGRATION_010).map_err(db_error)?;
        }
        if version <= 10 {
            connection.execute_batch(MIGRATION_011).map_err(db_error)?;
        }
        if version <= 11 {
            connection.execute_batch(MIGRATION_012).map_err(db_error)?;
        }
        if version <= 12 {
            connection.execute_batch(MIGRATION_013).map_err(db_error)?;
        }
        if version <= 13 {
            connection.execute_batch(MIGRATION_014).map_err(db_error)?;
        }
        if version <= 14 {
            connection.execute_batch(MIGRATION_015).map_err(db_error)?;
        }
        if version <= 15 {
            connection.execute_batch(MIGRATION_016).map_err(db_error)?;
        }
        if version <= 16 {
            connection.execute_batch(MIGRATION_017).map_err(db_error)?;
        }
        if version <= 17 {
            connection.execute_batch(MIGRATION_018).map_err(db_error)?;
        }
        if version <= 18 {
            connection.execute_batch(MIGRATION_019).map_err(db_error)?;
        }
        if version <= 19 {
            connection.execute_batch(MIGRATION_020).map_err(db_error)?;
        }
        if version <= 20 {
            connection.execute_batch(MIGRATION_021).map_err(db_error)?;
        }
        connection
            .execute_batch(ARCHIVE_USAGE_SQL)
            .map_err(db_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
            usage_revision: AtomicU64::new(0),
            api_equivalent_cache: Mutex::new(None),
            open_duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
        })
    }

    pub fn open_duration_ms(&self) -> f64 {
        self.open_duration_ms
    }

    pub fn record_performance(
        &self,
        name: &str,
        duration_ms: f64,
        context: Option<&str>,
    ) -> Result<()> {
        if !valid_performance_name(name)
            || !duration_ms.is_finite()
            || !(0.0..=600_000.0).contains(&duration_ms)
            || context.is_some_and(|value| {
                value.is_empty()
                    || value.len() > 64
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')
                    })
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "performance sample is invalid",
            ));
        }
        self.connection
            .lock()
            .map_err(lock_error)?
            .execute(
                "INSERT INTO performance_samples(name, duration_ms, context) VALUES (?1, ?2, ?3)",
                params![name, duration_ms, context],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub(crate) fn state_json(&self, key: &str) -> Result<Option<String>> {
        validate_state_key(key)?;
        self.connection
            .lock()
            .map_err(lock_error)?
            .query_row(
                "SELECT value_json FROM app_state WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)
    }

    pub(crate) fn state_count(&self) -> Result<usize> {
        let count: i64 = self
            .connection
            .lock()
            .map_err(lock_error)?
            .query_row("SELECT COUNT(*) FROM app_state", [], |row| row.get(0))
            .map_err(db_error)?;
        usize::try_from(count).map_err(|_| {
            LocalPoolError::new(ErrorCode::RecoveryRequired, "local state count is invalid")
        })
    }

    pub(crate) fn replace_state_json(&self, values: &[(&str, String)]) -> Result<()> {
        self.replace_state_json_with_account_purge(values, None)
    }

    pub(crate) fn replace_state_json_and_delete_account_data(
        &self,
        values: &[(&str, String)],
        account_id: &str,
    ) -> Result<()> {
        self.replace_state_json_with_account_purge(values, Some(account_id))
    }

    fn replace_state_json_with_account_purge(
        &self,
        values: &[(&str, String)],
        account_id: Option<&str>,
    ) -> Result<()> {
        for (key, value) in values {
            validate_state_key(key)?;
            if value.len() > MAX_STATE_JSON_BYTES {
                return Err(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "local state value is too large",
                ));
            }
        }
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (key, value) in values {
            transaction
                .execute(
                    "INSERT INTO app_state(key, value_json) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                    params![key, value],
                )
                .map_err(db_error)?;
        }
        if let Some(account_id) = account_id {
            transaction
                .execute(
                    "DELETE FROM request_logs WHERE account_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM response_affinity WHERE candidate_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        if account_id.is_some() {
            self.invalidate_usage_cache();
        }
        Ok(())
    }

    pub fn record(&self, event: &UsageEvent) -> Result<()> {
        if event.attempt == 0 {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "usage attempt must be at least one",
            ));
        }
        let routing_json = event
            .routing
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                LocalPoolError::new(
                    ErrorCode::Io,
                    format!("usage routing diagnostics serialization failed: {error}"),
                )
            })?;
        let tool_use_json = event
            .tool_use
            .has_evidence()
            .then(|| serde_json::to_string(&event.tool_use))
            .transpose()
            .map_err(|error| {
                LocalPoolError::new(
                    ErrorCode::Io,
                    format!("usage tool diagnostics serialization failed: {error}"),
                )
            })?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let changed = connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                    requested_model, resolved_model, wire_api, success, http_status,
                    error_category, latency_ms, ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
                ON CONFLICT(request_id) DO UPDATE SET
                    created_at = CURRENT_TIMESTAMP,
                    attempt = excluded.attempt,
                    local_key_id = excluded.local_key_id,
                    source_id = excluded.source_id,
                    candidate_id = excluded.candidate_id,
                    account_id = excluded.account_id,
                    requested_model = excluded.requested_model,
                    resolved_model = excluded.resolved_model,
                    wire_api = excluded.wire_api,
                    success = excluded.success,
                    http_status = excluded.http_status,
                    error_category = excluded.error_category,
                    latency_ms = excluded.latency_ms,
                    ttft_ms = excluded.ttft_ms,
                    generation_ms = excluded.generation_ms,
                    input_tokens = excluded.input_tokens,
                    cached_input_tokens = excluded.cached_input_tokens,
                    cache_write_input_tokens = excluded.cache_write_input_tokens,
                    reasoning_tokens = excluded.reasoning_tokens,
                    output_tokens = excluded.output_tokens,
                    total_tokens = excluded.total_tokens,
                    service_tier = excluded.service_tier,
                    applied_service_tier = excluded.applied_service_tier,
                    routing_json = excluded.routing_json,
                    tool_use_json = excluded.tool_use_json
                WHERE excluded.attempt >= request_logs.attempt",
                params![
                    event.request_id,
                    event.attempt,
                    event.local_key_id,
                    event.source_id,
                    event.candidate_id,
                    event.account_id,
                    event.requested_model,
                    event.resolved_model,
                    wire_api_name(event.wire_api),
                    event.success,
                    event.http_status,
                    event.error_category,
                    sql_u64(event.latency_ms),
                    event.ttft_ms.map(sql_u64),
                    event.generation_ms.map(sql_u64),
                    event.input_tokens.map(sql_u64),
                    event.cached_input_tokens.map(sql_u64),
                    event.cache_write_input_tokens.map(sql_u64),
                    event.reasoning_tokens.map(sql_u64),
                    event.output_tokens.map(sql_u64),
                    event.total_tokens.map(sql_u64),
                    service_tier_name(event.service_tier),
                    event.applied_service_tier.map(service_tier_name),
                    routing_json,
                    tool_use_json,
                ],
            )
            .map_err(db_error)?
            > 0;
        if connection.last_insert_rowid() % 256 == 0 {
            connection
                .execute_batch(ARCHIVE_USAGE_SQL)
                .map_err(db_error)?;
        }
        drop(connection);
        if changed {
            self.invalidate_usage_cache();
        }
        Ok(())
    }

    pub fn list(&self, limit: u16) -> Result<Vec<UsageLog>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                    local_key_id, source_id, candidate_id, account_id, requested_model,
                    resolved_model, wire_api, success, http_status, error_category, latency_ms,
                    ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json
                 FROM request_logs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(db_error)?;
        let logs = statement
            .query_map([limit.clamp(1, 500)], usage_log_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(logs)
    }

    #[cfg(test)]
    pub fn usage_page(&self, query: &UsageQuery) -> Result<LocalUsagePage> {
        self.usage_page_with_price_overrides(query, &BTreeMap::new(), &BTreeMap::new())
    }

    pub fn usage_page_with_price_overrides(
        &self,
        query: &UsageQuery,
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &SourcePriceOverrides,
    ) -> Result<LocalUsagePage> {
        let page = query.page.max(1);
        let page_size = if query.page_size == 0 {
            50
        } else {
            query.page_size.clamp(1, 200)
        };
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let (where_sql, values) = usage_filter(query);
        let mut totals = usage_totals(&connection, &where_sql, &values)?;
        let mut models = usage_groups(
            &connection,
            &where_sql,
            &values,
            "COALESCE(resolved_model, requested_model, '')",
        )?;
        let mut model_equivalents = usage_model_equivalents(
            &connection,
            &where_sql,
            &values,
            price_overrides,
            source_price_overrides,
        )?;
        for group in &mut models {
            group.totals.api_equivalent = model_equivalents.remove(&group.key).unwrap_or_default();
            totals.api_equivalent.merge(group.totals.api_equivalent);
        }
        let pool_members = usage_groups(
            &connection,
            &where_sql,
            &values,
            "COALESCE(account_id, source_id, '')",
        )?;
        let buckets = usage_buckets(
            &connection,
            &where_sql,
            &values,
            query,
            price_overrides,
            source_price_overrides,
        )?;
        let total = totals.requests;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let sql = format!(
            "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                local_key_id, source_id, candidate_id, account_id, requested_model,
                resolved_model, wire_api, success, http_status, error_category, latency_ms,
                ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                service_tier, applied_service_tier, routing_json, tool_use_json
             FROM request_logs{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let mut page_values = values;
        page_values.push(SqlValue::Integer(i64::from(page_size)));
        page_values.push(SqlValue::Integer(offset.min(i64::MAX as u64) as i64));
        let mut events = statement
            .query_map(params_from_iter(page_values.iter()), usage_log_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        for event in &mut events {
            let candidate_kind = if event.account_id.is_some() {
                "account"
            } else {
                "source"
            };
            let candidate_id = event.account_id.as_deref().unwrap_or(&event.source_id);
            let model = event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref());
            event.api_equivalent = estimate_api_equivalent_with_price_override(
                model,
                event.input_tokens,
                event.cached_input_tokens,
                event.cache_write_input_tokens,
                event.output_tokens,
                event.total_tokens,
                configured_model_price(
                    price_overrides,
                    source_price_overrides,
                    candidate_kind,
                    candidate_id,
                    model,
                ),
            );
        }
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(u64::from(page_size)) as u32
        };
        Ok(LocalUsagePage {
            events,
            total,
            page,
            page_size,
            total_pages,
            totals,
            models,
            pool_members,
            buckets,
        })
    }

    #[cfg(test)]
    pub fn api_equivalents(&self) -> Result<UsageEquivalents> {
        self.api_equivalents_with_price_overrides(&BTreeMap::new(), &BTreeMap::new())
    }

    pub fn api_equivalents_with_price_overrides(
        &self,
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &SourcePriceOverrides,
    ) -> Result<UsageEquivalents> {
        let usage_revision = self.usage_revision.load(Ordering::Acquire);
        let pricing_revision = serde_json::to_string(&(
            api_pricing_revision(),
            price_overrides,
            source_price_overrides,
        ))
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("usage pricing revision serialization failed: {error}"),
            )
        })?;
        if let Some(cached) = self
            .api_equivalent_cache
            .lock()
            .map_err(lock_error)?
            .as_ref()
            .filter(|cached| {
                cached.usage_revision == usage_revision
                    && cached.pricing_revision == pricing_revision
            })
        {
            return Ok(cached.value.clone());
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT candidate_kind, candidate_id, model,
                    SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
                    SUM(output_tokens), SUM(total_tokens), SUM(input_samples),
                    SUM(cached_input_samples), SUM(cache_write_input_samples)
                 FROM (
                    SELECT candidate_kind, candidate_id, model,
                        input_tokens, cached_input_tokens, cache_write_input_tokens,
                        output_tokens, total_tokens, input_samples,
                        cached_input_samples, cache_write_input_samples
                    FROM usage_candidate_rollups
                    UNION ALL
                    SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                        COALESCE(account_id, source_id),
                        COALESCE(resolved_model, requested_model, ''),
                        COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
                        COALESCE(SUM(cache_write_input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(total_tokens), 0), COUNT(input_tokens),
                        COUNT(cached_input_tokens), COUNT(cache_write_input_tokens)
                    FROM request_logs GROUP BY 1, 2, 3
                 ) GROUP BY candidate_kind, candidate_id, model",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let input_tokens: Option<i64> = row.get(3)?;
                let cached_input_tokens: Option<i64> = row.get(4)?;
                let cache_write_input_tokens: Option<i64> = row.get(5)?;
                let output_tokens: Option<i64> = row.get(6)?;
                let total_tokens: Option<i64> = row.get(7)?;
                let input_samples: i64 = row.get(8)?;
                let cached_samples: i64 = row.get(9)?;
                let cache_write_samples: i64 = row.get(10)?;
                let model = row.get::<_, Option<String>>(2)?;
                let kind = row.get::<_, String>(0)?;
                let id = row.get::<_, String>(1)?;
                Ok((
                    kind.clone(),
                    id.clone(),
                    estimate_api_equivalent_with_price_override(
                        model.as_deref(),
                        input_tokens.map(rust_u64),
                        (input_samples > 0 && cached_samples == input_samples)
                            .then(|| cached_input_tokens.map(rust_u64))
                            .flatten(),
                        (input_samples > 0 && cache_write_samples == input_samples)
                            .then(|| cache_write_input_tokens.map(rust_u64))
                            .flatten(),
                        output_tokens.map(rust_u64),
                        total_tokens.map(rust_u64),
                        configured_model_price(
                            price_overrides,
                            source_price_overrides,
                            &kind,
                            &id,
                            model.as_deref(),
                        ),
                    ),
                ))
            })
            .map_err(db_error)?;
        let mut equivalents = UsageEquivalents::default();
        for row in rows {
            let (kind, id, estimate) = row.map_err(db_error)?;
            let values = if kind == "account" {
                &mut equivalents.accounts
            } else {
                &mut equivalents.sources
            };
            values.entry(id).or_default().merge(estimate);
        }
        drop(statement);
        drop(connection);
        if self.usage_revision.load(Ordering::Acquire) == usage_revision {
            self.api_equivalent_cache
                .lock()
                .map_err(lock_error)?
                .replace(CachedUsageEquivalents {
                    usage_revision,
                    pricing_revision,
                    value: equivalents.clone(),
                });
        }
        Ok(equivalents)
    }

    pub fn clear(&self) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute_batch("DELETE FROM request_logs; DELETE FROM usage_candidate_rollups;")
            .map_err(db_error)?;
        self.invalidate_usage_cache();
        Ok(())
    }

    fn invalidate_usage_cache(&self) {
        self.usage_revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cached) = self.api_equivalent_cache.lock() {
            *cached = None;
        }
    }

    pub fn affinity_bindings(&self, now_ms: u64) -> Result<Vec<ResponseAffinityBinding>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        connection
            .execute(
                "DELETE FROM response_affinity WHERE expires_at_ms <= ?1",
                [sql_u64(now_ms)],
            )
            .map_err(db_error)?;
        let mut statement = connection
            .prepare(
                "SELECT response_key, candidate_id, expires_at_ms
                 FROM response_affinity
                 ORDER BY updated_at_ms DESC, response_key DESC
                 LIMIT ?1",
            )
            .map_err(db_error)?;
        let bindings = statement
            .query_map(
                [MAX_RESPONSE_AFFINITY_ROWS as i64],
                affinity_binding_from_row,
            )
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(bindings)
    }

    pub fn find_affinity(&self, key: &str, now_ms: u64) -> Result<Option<ResponseAffinityBinding>> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .query_row(
                "SELECT response_key, candidate_id, expires_at_ms
                 FROM response_affinity WHERE response_key = ?1 AND expires_at_ms > ?2",
                params![key, sql_u64(now_ms)],
                affinity_binding_from_row,
            )
            .optional()
            .map_err(db_error)
    }

    pub fn upsert_affinity(&self, binding: &ResponseAffinityBinding, now_ms: u64) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "INSERT INTO response_affinity(response_key, candidate_id, expires_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(response_key) DO UPDATE SET
                    candidate_id = excluded.candidate_id,
                    expires_at_ms = excluded.expires_at_ms,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    binding.key,
                    binding.candidate_id,
                    sql_u64(binding.expires_at_ms),
                    sql_u64(now_ms),
                ],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn delete_affinity(&self, key: &str) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "DELETE FROM response_affinity WHERE response_key = ?1",
                [key],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn delete_candidate_affinities(&self, candidate_id: &str) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "DELETE FROM response_affinity WHERE candidate_id = ?1",
                [candidate_id],
            )
            .map(|_| ())
            .map_err(db_error)
    }
}

fn affinity_binding_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResponseAffinityBinding> {
    let expires_at_ms: i64 = row.get(2)?;
    Ok(ResponseAffinityBinding {
        key: row.get(0)?,
        candidate_id: row.get(1)?,
        expires_at_ms: rust_u64(expires_at_ms),
    })
}

const USAGE_TOTAL_COLUMNS: &str = "COUNT(*), \
    COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
    COALESCE(SUM(latency_ms), 0), COALESCE(SUM(ttft_ms), 0), COUNT(ttft_ms), \
    COALESCE(SUM(CASE WHEN success != 0 THEN generation_ms ELSE 0 END), 0), \
    COUNT(CASE WHEN success != 0 THEN generation_ms END), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms IS NOT NULL \
        THEN MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0), 0) ELSE 0 END), 0), \
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0), \
    COUNT(cached_input_tokens), COALESCE(SUM(cache_write_input_tokens), 0), \
    COUNT(cache_write_input_tokens), COALESCE(SUM(reasoning_tokens), 0), \
    COALESCE(SUM(output_tokens), 0), \
    COALESCE(SUM(total_tokens), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > COALESCE(reasoning_tokens, 0) \
        THEN output_tokens - COALESCE(reasoning_tokens, 0) ELSE 0 END), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > COALESCE(reasoning_tokens, 0) AND latency_ms > 0 \
        THEN latency_ms ELSE 0 END), 0)";

fn usage_filter(query: &UsageQuery) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(value) = query.from_ms {
        clauses.push("created_at >= datetime(? / 1000, 'unixepoch')");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.to_ms {
        clauses.push("created_at <= datetime(? / 1000, 'unixepoch')");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.model_query.as_deref() {
        clauses.push("(requested_model LIKE ? ESCAPE '\\' OR resolved_model LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(like_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.source_or_account_query.as_deref() {
        clauses.push("(source_id LIKE ? ESCAPE '\\' OR account_id LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(like_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.wire_api {
        match value {
            WireApi::ChatCompletions => {
                clauses.push("wire_api IN (?, ?)");
                values.push(SqlValue::Text("chat_completions".to_string()));
                values.push(SqlValue::Text("chatcompletions".to_string()));
            }
            _ => {
                clauses.push("wire_api = ?");
                values.push(SqlValue::Text(wire_api_name(value).to_string()));
            }
        }
    }
    if let Some(value) = query.success {
        clauses.push("success = ?");
        values.push(SqlValue::Integer(i64::from(value)));
    }
    if let Some(value) = query.error_category.as_deref() {
        clauses.push("error_category = ?");
        values.push(SqlValue::Text(value.to_string()));
    }
    if let Some(value) = query.request_id_query.as_deref() {
        clauses.push("request_id LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, values)
}

fn usage_totals(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
) -> Result<UsageTotals> {
    let sql = format!("SELECT {USAGE_TOTAL_COLUMNS} FROM request_logs{where_sql}");
    connection
        .query_row(&sql, params_from_iter(values.iter()), |row| {
            usage_totals_from_row(row, 0)
        })
        .map_err(db_error)
}

fn usage_groups(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    key_sql: &str,
) -> Result<Vec<UsageGroup>> {
    let sql = format!(
        "SELECT {key_sql}, {USAGE_TOTAL_COLUMNS} FROM request_logs{where_sql} \
         GROUP BY 1 ORDER BY COUNT(*) DESC, 1"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok(UsageGroup {
                key: row.get(0)?,
                label: None,
                totals: usage_totals_from_row(row, 1)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)
}

fn usage_model_equivalents(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> Result<HashMap<String, ApiEquivalentSummary>> {
    let sql = format!(
        "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
            COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens),
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens),
            COUNT(output_tokens), COUNT(total_tokens)
         FROM request_logs{where_sql} GROUP BY 1, 2, 3"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            let kind = row.get::<_, String>(0)?;
            let candidate_id = row.get::<_, String>(1)?;
            let model = row.get::<_, String>(2)?;
            let input_tokens = row.get::<_, Option<i64>>(3)?.map(rust_u64);
            let cached_input_tokens = row.get::<_, Option<i64>>(4)?.map(rust_u64);
            let cache_write_input_tokens = row.get::<_, Option<i64>>(5)?.map(rust_u64);
            let output_tokens = row.get::<_, Option<i64>>(6)?.map(rust_u64);
            let total_tokens = row.get::<_, Option<i64>>(7)?.map(rust_u64);
            let input_samples = rust_u64(row.get(8)?);
            let cached_samples = rust_u64(row.get(9)?);
            let cache_write_samples = rust_u64(row.get(10)?);
            let output_samples = rust_u64(row.get(11)?);
            let total_samples = rust_u64(row.get(12)?);
            Ok((
                model.clone(),
                estimate_api_equivalent_with_price_override(
                    (!model.is_empty()).then_some(model.as_str()),
                    (input_samples > 0).then_some(input_tokens).flatten(),
                    (input_samples > 0 && cached_samples == input_samples)
                        .then_some(cached_input_tokens)
                        .flatten(),
                    (input_samples > 0 && cache_write_samples == input_samples)
                        .then_some(cache_write_input_tokens)
                        .flatten(),
                    (output_samples > 0).then_some(output_tokens).flatten(),
                    (total_samples > 0).then_some(total_tokens).flatten(),
                    configured_model_price(
                        price_overrides,
                        source_price_overrides,
                        &kind,
                        &candidate_id,
                        (!model.is_empty()).then_some(model.as_str()),
                    ),
                ),
            ))
        })
        .map_err(db_error)?;
    let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
    for row in rows {
        let (model, estimate) = row.map_err(db_error)?;
        equivalents.entry(model).or_default().merge(estimate);
    }
    Ok(equivalents)
}

fn usage_buckets(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    query: &UsageQuery,
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> Result<Vec<UsageBucket>> {
    let Some(bucket_ms) = query.bucket_ms else {
        return Ok(Vec::new());
    };
    let start_ms = query.from_ms.unwrap_or_default();
    let start = SqlValue::Integer(start_ms.min(i64::MAX as u64) as i64);
    let bucket = SqlValue::Integer(bucket_ms.min(i64::MAX as u64) as i64);
    let bucket_sql = "? + ((CAST(strftime('%s', created_at) AS INTEGER) * 1000 - ?) / ?) * ?";
    let sql = format!(
        "SELECT {bucket_sql}, {USAGE_TOTAL_COLUMNS} \
         FROM request_logs{where_sql} GROUP BY 1 ORDER BY 1"
    );
    let mut parameters = vec![start.clone(), start, bucket.clone(), bucket];
    parameters.extend_from_slice(values);
    let mut buckets = {
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok(UsageBucket {
                    start_ms: rust_u64(row.get(0)?),
                    totals: usage_totals_from_row(row, 1)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?
    };
    let price_sql = format!(
        "SELECT {bucket_sql}, CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END, \
            COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model), \
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens), \
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens), \
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens), \
            COUNT(output_tokens), COUNT(total_tokens) \
         FROM request_logs{where_sql} GROUP BY 1, 2, 3, 4"
    );
    let mut statement = connection.prepare(&price_sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            let kind = row.get::<_, String>(1)?;
            let candidate_id = row.get::<_, String>(2)?;
            let model = row.get::<_, Option<String>>(3)?;
            let input_tokens: Option<i64> = row.get(4)?;
            let cached_input_tokens: Option<i64> = row.get(5)?;
            let cache_write_input_tokens: Option<i64> = row.get(6)?;
            let output_tokens: Option<i64> = row.get(7)?;
            let total_tokens: Option<i64> = row.get(8)?;
            let input_samples: i64 = row.get(9)?;
            let cached_samples: i64 = row.get(10)?;
            let cache_write_samples: i64 = row.get(11)?;
            let output_samples: i64 = row.get(12)?;
            let total_samples: i64 = row.get(13)?;
            let start_ms = rust_u64(row.get(0)?);
            let input_tokens = (input_samples > 0)
                .then(|| input_tokens.map(rust_u64))
                .flatten();
            let cached_input_tokens = (input_samples > 0 && cached_samples == input_samples)
                .then(|| cached_input_tokens.map(rust_u64))
                .flatten();
            let cache_write_input_tokens = (input_samples > 0
                && cache_write_samples == input_samples)
                .then(|| cache_write_input_tokens.map(rust_u64))
                .flatten();
            Ok((
                start_ms,
                estimate_api_equivalent_with_price_override(
                    model.as_deref(),
                    input_tokens,
                    cached_input_tokens,
                    cache_write_input_tokens,
                    (output_samples > 0)
                        .then(|| output_tokens.map(rust_u64))
                        .flatten(),
                    (total_samples > 0)
                        .then(|| total_tokens.map(rust_u64))
                        .flatten(),
                    configured_model_price(
                        price_overrides,
                        source_price_overrides,
                        &kind,
                        &candidate_id,
                        model.as_deref(),
                    ),
                ),
            ))
        })
        .map_err(db_error)?;
    let mut equivalents = HashMap::<u64, ApiEquivalentSummary>::new();
    for row in rows {
        let (start_ms, estimate) = row.map_err(db_error)?;
        equivalents.entry(start_ms).or_default().merge(estimate);
    }
    for bucket in &mut buckets {
        bucket.totals.api_equivalent = equivalents.remove(&bucket.start_ms).unwrap_or_default();
    }
    Ok(buckets)
}

fn configured_model_price(
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
    candidate_kind: &str,
    candidate_id: &str,
    model: Option<&str>,
) -> Option<ApiModelPriceOverride> {
    let model = model?.to_ascii_lowercase();
    (candidate_kind == "source")
        .then(|| {
            source_price_overrides
                .get(candidate_id)?
                .get(&model)
                .copied()
        })
        .flatten()
        .or_else(|| price_overrides.get(&model).copied())
}

fn usage_totals_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        requests: rust_u64(row.get(offset)?),
        successful_requests: rust_u64(row.get(offset + 1)?),
        latency_ms: rust_u64(row.get(offset + 2)?),
        ttft_ms: rust_u64(row.get(offset + 3)?),
        ttft_samples: rust_u64(row.get(offset + 4)?),
        generation_ms: rust_u64(row.get(offset + 5)?),
        generation_samples: rust_u64(row.get(offset + 6)?),
        generation_output_tokens: rust_u64(row.get(offset + 7)?),
        input_tokens: rust_u64(row.get(offset + 8)?),
        cached_input_tokens: rust_u64(row.get(offset + 9)?),
        cached_input_samples: rust_u64(row.get(offset + 10)?),
        cache_write_input_tokens: rust_u64(row.get(offset + 11)?),
        cache_write_input_samples: rust_u64(row.get(offset + 12)?),
        reasoning_tokens: rust_u64(row.get(offset + 13)?),
        output_tokens: rust_u64(row.get(offset + 14)?),
        total_tokens: rust_u64(row.get(offset + 15)?),
        speed_output_tokens: rust_u64(row.get(offset + 16)?),
        speed_duration_ms: rust_u64(row.get(offset + 17)?),
        api_equivalent: ApiEquivalentSummary::default(),
    })
}

fn usage_log_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageLog> {
    let latency_ms: i64 = row.get(14)?;
    let ttft_ms: Option<i64> = row.get(15)?;
    let generation_ms: Option<i64> = row.get(16)?;
    let input_tokens: Option<i64> = row.get(17)?;
    let cached_input_tokens: Option<i64> = row.get(18)?;
    let cache_write_input_tokens: Option<i64> = row.get(19)?;
    let reasoning_tokens: Option<i64> = row.get(20)?;
    let output_tokens: Option<i64> = row.get(21)?;
    let total_tokens: Option<i64> = row.get(22)?;
    let service_tier: String = row.get(23)?;
    let applied_service_tier: Option<String> = row.get(24)?;
    let routing_json: Option<String> = row.get(25)?;
    let tool_use_json: Option<String> = row.get(26)?;
    Ok(UsageLog {
        id: row.get(0)?,
        created_at: row.get(1)?,
        request_id: row.get(2)?,
        attempt: row.get(3)?,
        source_id: row.get(5)?,
        candidate_id: row.get(6)?,
        account_id: row.get(7)?,
        routing: routing_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        requested_model: row.get(8)?,
        resolved_model: row.get(9)?,
        wire_api: normalize_wire_api(row.get(10)?),
        service_tier: parse_service_tier(&service_tier),
        applied_service_tier: applied_service_tier.as_deref().map(parse_service_tier),
        success: row.get(11)?,
        http_status: row.get(12)?,
        error_category: row.get(13)?,
        tool_use: tool_use_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        latency_ms: rust_u64(latency_ms),
        ttft_ms: ttft_ms.map(rust_u64),
        generation_ms: generation_ms.map(rust_u64),
        input_tokens: input_tokens.map(rust_u64),
        cached_input_tokens: cached_input_tokens.map(rust_u64),
        cache_write_input_tokens: cache_write_input_tokens.map(rust_u64),
        reasoning_tokens: reasoning_tokens.map(rust_u64),
        output_tokens: output_tokens.map(rust_u64),
        total_tokens: total_tokens.map(rust_u64),
        api_equivalent: ApiEquivalentSummary::default(),
    })
}

fn like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

fn wire_api_name(value: WireApi) -> &'static str {
    match value {
        WireApi::Responses => "responses",
        WireApi::ChatCompletions => "chat_completions",
        WireApi::Messages => "messages",
    }
}

fn service_tier_name(value: DefaultServiceTier) -> &'static str {
    match value {
        DefaultServiceTier::Standard => "standard",
        DefaultServiceTier::Fast => "fast",
    }
}

fn parse_service_tier(value: &str) -> DefaultServiceTier {
    if value.eq_ignore_ascii_case("fast") || value.eq_ignore_ascii_case("priority") {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

fn normalize_wire_api(value: String) -> String {
    if value == "chatcompletions" {
        "chat_completions".to_string()
    } else {
        value
    }
}

fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn rust_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn valid_performance_name(name: &str) -> bool {
    matches!(
        name,
        "native_startup"
            | "vault"
            | "sqlite"
            | "window"
            | "first_frame"
            | "interactive"
            | "full_snapshot"
            | "full_snapshot_native"
            | "mode_switch"
            | "page_open"
    )
}

fn validate_state_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "local state key is invalid",
        ));
    }
    Ok(())
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, "local database lock poisoned")
}

fn db_error(error: rusqlite::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, format!("local database error: {error}"))
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{SelectionReason, TerminalOutputKind, ToolChoiceMode, WireApi};

    #[test]
    fn usage_survives_database_reopen() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-{}", uuid::Uuid::new_v4()));
        let path = root.join("usage.sqlite");
        let event = UsageEvent {
            request_id: "req_1".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            candidate_id: Some("account_1".into()),
            account_id: Some("account_1".into()),
            routing: Some(RoutingDiagnostics {
                reason: SelectionReason::QuotaHeadroom,
                eligible_candidates: 4,
                quota_remaining_basis_points: Some(6_300),
                in_flight_before: 0,
                dispatches_before: 3,
            }),
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Fast,
            applied_service_tier: Some(DefaultServiceTier::Standard),
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics {
                client_tool_count: 3,
                forwarded_tool_count: 3,
                tool_choice: ToolChoiceMode::Auto,
                tool_call_count: 1,
                text_output: false,
                terminal_output: TerminalOutputKind::ToolCall,
            },
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 12,
            ttft_ms: Some(4),
            generation_ms: Some(8),
            input_tokens: Some(2),
            cached_input_tokens: Some(1),
            cache_write_input_tokens: Some(1),
            reasoning_tokens: Some(2),
            output_tokens: Some(3),
            total_tokens: Some(5),
            quota_snapshot: None,
        };
        TelemetryDb::open(&path).unwrap().record(&event).unwrap();
        let database = TelemetryDb::open(&path).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].created_at.ends_with('Z'));
        assert_eq!(logs[0].candidate_id.as_deref(), Some("account_1"));
        assert_eq!(logs[0].ttft_ms, Some(4));
        assert_eq!(logs[0].cached_input_tokens, Some(1));
        assert_eq!(logs[0].cache_write_input_tokens, Some(1));
        assert_eq!(logs[0].reasoning_tokens, Some(2));
        assert_eq!(
            logs[0]
                .tool_use
                .as_ref()
                .map(|tool_use| tool_use.tool_call_count),
            Some(1)
        );
        assert_eq!(logs[0].service_tier, DefaultServiceTier::Fast);
        assert_eq!(
            logs[0].applied_service_tier,
            Some(DefaultServiceTier::Standard)
        );
        assert_eq!(
            logs[0].routing.as_ref().map(|routing| routing.reason),
            Some(SelectionReason::QuotaHeadroom)
        );
        let page = database.usage_page(&UsageQuery::default()).unwrap();
        // The event carries a measured value and the totals are that same value
        // merged once, so the relation holds regardless of catalog prices.
        assert!(page.events[0].api_equivalent.micro_usd > 0);
        assert_eq!(page.totals.api_equivalent, page.events[0].api_equivalent);
        let cached = database.api_equivalents().unwrap();
        assert_eq!(
            database.api_equivalents().unwrap().accounts,
            cached.accounts
        );
        let mut second = event.clone();
        second.request_id = "req_2".into();
        second.input_tokens = Some(20);
        second.total_tokens = Some(23);
        database.record(&second).unwrap();
        assert!(
            database.api_equivalents().unwrap().accounts["account_1"].micro_usd
                > cached.accounts["account_1"].micro_usd
        );
        database
            .record_performance("first_frame", 12.5, Some("startup"))
            .unwrap();
        assert!(database
            .record_performance("unknown_metric", 1.0, None)
            .is_err());
        let performance_samples: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM performance_samples", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(performance_samples, 1);
        database.clear().unwrap();
        assert!(database.list(10).unwrap().is_empty());
        assert_eq!(logs[0].total_tokens, Some(5));
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn response_affinity_survives_reopen_and_expires() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-affinity-{}", uuid::Uuid::new_v4()));
        let path = root.join("usage.sqlite");
        let binding = ResponseAffinityBinding {
            key: "hashed-response".into(),
            candidate_id: "account-1".into(),
            expires_at_ms: 200,
        };
        TelemetryDb::open(&path)
            .unwrap()
            .upsert_affinity(&binding, 100)
            .unwrap();
        let database = TelemetryDb::open(&path).unwrap();
        assert_eq!(
            database.find_affinity(&binding.key, 199).unwrap(),
            Some(binding)
        );
        assert!(database.affinity_bindings(200).unwrap().is_empty());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_account_data_removes_usage_rollups_and_affinity() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-account-delete-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO request_logs(
                    request_id, local_key_id, source_id, candidate_id, account_id,
                    wire_api, success, http_status, latency_ms
                 ) VALUES ('request-delete', 'key', 'codex', 'account-delete',
                    'account-delete', 'responses', 1, 200, 1);
                 INSERT INTO usage_candidate_rollups(candidate_kind, candidate_id, model)
                 VALUES ('account', 'account-delete', 'gpt-test');
                 INSERT INTO response_affinity(
                    response_key, candidate_id, expires_at_ms, updated_at_ms
                 ) VALUES ('response-delete', 'account-delete', 1000, 1);",
            )
            .unwrap();

        database
            .replace_state_json_and_delete_account_data(
                &[("accounts", "[]".to_string())],
                "account-delete",
            )
            .unwrap();

        let remaining: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM request_logs WHERE account_id = 'account-delete') +
                    (SELECT COUNT(*) FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id = 'account-delete') +
                    (SELECT COUNT(*) FROM response_affinity
                     WHERE candidate_id = 'account-delete')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(
            database.state_json("accounts").unwrap().as_deref(),
            Some("[]")
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn response_affinity_storage_matches_the_runtime_capacity() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-affinity-capacity-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "WITH RECURSIVE entries(value) AS (
                    SELECT 0 UNION ALL SELECT value + 1 FROM entries WHERE value < {limit}
                 )
                 INSERT INTO response_affinity(
                    response_key, candidate_id, expires_at_ms, updated_at_ms
                 )
                 SELECT printf('response-%05d', value), 'account-1', 999999, value
                 FROM entries;",
                limit = MAX_RESPONSE_AFFINITY_ROWS + 1
            ))
            .unwrap();

        let bindings = database.affinity_bindings(0).unwrap();
        assert_eq!(bindings.len(), MAX_RESPONSE_AFFINITY_ROWS);
        assert_eq!(
            bindings.first().map(|binding| binding.key.as_str()),
            Some("response-04097")
        );
        assert_eq!(
            bindings.last().map(|binding| binding.key.as_str()),
            Some("response-00002")
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_page_aggregates_the_full_filtered_range_not_only_the_page() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-page-{}", uuid::Uuid::new_v4()));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let mut event = UsageEvent {
            request_id: "req_page_1".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "openai-codex".into(),
            candidate_id: Some("account_1".into()),
            account_id: Some("account_1".into()),
            routing: None,
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 428,
            ttft_ms: Some(128),
            generation_ms: Some(300),
            input_tokens: Some(20),
            cached_input_tokens: Some(12),
            cache_write_input_tokens: None,
            reasoning_tokens: Some(5),
            output_tokens: Some(8),
            total_tokens: Some(28),
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.request_id = "req_page_2".into();
        event.candidate_id = Some("account_2".into());
        event.account_id = Some("account_2".into());
        event.wire_api = WireApi::ChatCompletions;
        event.latency_ms = 500;
        event.ttft_ms = Some(100);
        event.input_tokens = Some(10);
        event.cached_input_tokens = Some(0);
        event.reasoning_tokens = Some(0);
        event.output_tokens = Some(20);
        event.total_tokens = Some(30);
        database.record(&event).unwrap();
        event.request_id = "req_page_3".into();
        event.success = false;
        event.http_status = 502;
        event.error_category = Some("upstream_websocket_closed".into());
        event.generation_ms = Some(5_000);
        event.input_tokens = Some(0);
        event.output_tokens = Some(100);
        event.total_tokens = Some(100);
        database.record(&event).unwrap();

        let page = database
            .usage_page(&UsageQuery {
                page: 1,
                page_size: 1,
                from_ms: Some(0),
                bucket_ms: Some(3_600_000),
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.total, 3);
        assert_eq!(page.total_pages, 3);
        assert_eq!(page.totals.requests, 3);
        assert_eq!(page.totals.total_tokens, 158);
        assert_eq!(page.totals.generation_output_tokens, 23);
        assert_eq!(page.totals.generation_ms, 600);
        assert_eq!(page.totals.generation_samples, 2);
        assert_eq!(page.totals.speed_output_tokens, 23);
        assert_eq!(page.totals.speed_duration_ms, 928);
        assert_eq!(page.totals.api_equivalent.priced_tokens, 158);
        assert_eq!(page.models.len(), 1);
        assert_eq!(page.pool_members.len(), 2);
        assert_eq!(page.buckets.len(), 1);
        assert_eq!(page.buckets[0].totals.total_tokens, 158);
        assert_eq!(
            page.buckets[0].totals.api_equivalent,
            page.totals.api_equivalent
        );
        assert_eq!(page.events[0].wire_api, "chat_completions");
        assert_eq!(page.events[0].service_tier, DefaultServiceTier::Standard);
        assert!(page.events[0].tool_use.is_none());

        let chat = database
            .usage_page(&UsageQuery {
                wire_api: Some(WireApi::ChatCompletions),
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(chat.total, 2);
        assert_eq!(chat.events[0].request_id, "req_page_3");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_keeps_only_the_terminal_fallback_attempt() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-attempts-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("usage.sqlite");
        let database = TelemetryDb::open(&path).unwrap();
        let mut event = UsageEvent {
            request_id: "req_fallback".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            candidate_id: Some("source_1".into()),
            account_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status: 503,
            error_category: Some("upstream_unavailable".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("*".into()),
            retry_at_ms: Some(60_000),
            consecutive_failures: Some(1),
            latency_ms: 5,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.attempt = 2;
        event.source_id = "source_2".into();
        event.candidate_id = Some("source_2".into());
        event.success = true;
        event.http_status = 200;
        event.error_category = None;
        event.cooldown_scope = None;
        event.retry_at_ms = None;
        event.consecutive_failures = Some(0);
        database.record(&event).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 2);
        assert!(logs[0].success);
        assert_eq!(logs[0].source_id, "source_2");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_keeps_only_the_last_failure_when_all_attempts_fail() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-failed-attempts-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let mut event = UsageEvent {
            request_id: "req_failed".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            candidate_id: Some("source_1".into()),
            account_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status: 503,
            error_category: Some("upstream_unavailable".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("*".into()),
            retry_at_ms: Some(60_000),
            consecutive_failures: Some(1),
            latency_ms: 5,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.attempt = 2;
        event.source_id = "source_2".into();
        event.candidate_id = Some("source_2".into());
        event.http_status = 429;
        event.error_category = Some("upstream_rate_limited".into());
        database.record(&event).unwrap();

        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 2);
        assert!(!logs[0].success);
        assert_eq!(logs[0].http_status, 429);
        assert_eq!(logs[0].source_id, "source_2");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn api_equivalents_group_priced_and_unknown_usage_by_candidate() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-equivalent-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let event = UsageEvent {
            request_id: "req_equivalent".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            candidate_id: Some("account_1".into()),
            account_id: Some("account_1".into()),
            routing: None,
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 12,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(20),
            cached_input_tokens: Some(10),
            cache_write_input_tokens: None,
            reasoning_tokens: Some(3),
            output_tokens: Some(8),
            total_tokens: Some(28),
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        let equivalents = database.api_equivalents().unwrap();
        assert_eq!(
            equivalents.accounts.get("account_1"),
            Some(&ApiEquivalentSummary {
                micro_usd: 148,
                priced_tokens: 28,
                unpriced_tokens: 0,
            })
        );
        assert!(equivalents.sources.is_empty());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_price_revalues_existing_unknown_model_usage() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-custom-price-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        database
            .record(&UsageEvent {
                request_id: "req_custom_price".into(),
                attempt: 1,
                local_key_id: "key_1".into(),
                source_id: "source_1".into(),
                candidate_id: Some("source_1".into()),
                account_id: None,
                routing: None,
                requested_model: Some("private-model".into()),
                resolved_model: Some("private-model".into()),
                wire_api: WireApi::Responses,
                service_tier: DefaultServiceTier::Standard,
                applied_service_tier: None,
                success: true,
                http_status: 200,
                error_category: None,
                tool_use: ToolUseDiagnostics::default(),
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: Some(0),
                latency_ms: 100,
                ttft_ms: Some(10),
                generation_ms: Some(90),
                input_tokens: Some(1_000_000),
                cached_input_tokens: Some(0),
                cache_write_input_tokens: Some(0),
                reasoning_tokens: Some(0),
                output_tokens: Some(100_000),
                total_tokens: Some(1_100_000),
                quota_snapshot: None,
            })
            .unwrap();

        assert_eq!(
            database
                .usage_page(&UsageQuery::default())
                .unwrap()
                .totals
                .api_equivalent
                .unpriced_tokens,
            1_100_000
        );
        let prices = BTreeMap::from([(
            "private-model".into(),
            ApiModelPriceOverride {
                input_micro_usd_per_million: 2_000_000,
                cached_input_micro_usd_per_million: Some(200_000),
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 10_000_000,
            },
        )]);
        let page = database
            .usage_page_with_price_overrides(&UsageQuery::default(), &prices, &BTreeMap::new())
            .unwrap();
        assert_eq!(page.totals.api_equivalent.micro_usd, 3_000_000);
        assert_eq!(page.totals.api_equivalent.priced_tokens, 1_100_000);
        assert_eq!(page.totals.api_equivalent.unpriced_tokens, 0);
        assert_eq!(page.events[0].api_equivalent, page.totals.api_equivalent);
        assert_eq!(
            database
                .api_equivalents_with_price_overrides(&prices, &BTreeMap::new())
                .unwrap()
                .sources
                .get("source_1")
                .map(|summary| summary.micro_usd),
            Some(3_000_000)
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_prices_are_applied_before_same_model_usage_is_merged() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-source-prices-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let mut event = UsageEvent {
            request_id: "req_source_cheap".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_cheap".into(),
            candidate_id: Some("source_cheap".into()),
            account_id: None,
            routing: None,
            requested_model: Some("private-model".into()),
            resolved_model: Some("private-model".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 100,
            ttft_ms: Some(10),
            generation_ms: Some(90),
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(0),
            cache_write_input_tokens: Some(0),
            reasoning_tokens: Some(0),
            output_tokens: Some(100_000),
            total_tokens: Some(1_100_000),
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.request_id = "req_source_expensive".into();
        event.source_id = "source_expensive".into();
        event.candidate_id = Some("source_expensive".into());
        database.record(&event).unwrap();
        event.request_id = "req_account".into();
        event.source_id = "codex".into();
        event.candidate_id = Some("account_1".into());
        event.account_id = Some("account_1".into());
        database.record(&event).unwrap();

        let price = |input, output| ApiModelPriceOverride {
            input_micro_usd_per_million: input,
            cached_input_micro_usd_per_million: Some(input / 10),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: output,
        };
        let source_prices = BTreeMap::from([
            (
                "source_cheap".into(),
                BTreeMap::from([("private-model".into(), price(1_000_000, 2_000_000))]),
            ),
            (
                "source_expensive".into(),
                BTreeMap::from([("private-model".into(), price(2_000_000, 4_000_000))]),
            ),
        ]);
        let page = database
            .usage_page_with_price_overrides(
                &UsageQuery::default(),
                &BTreeMap::new(),
                &source_prices,
            )
            .unwrap();
        assert_eq!(page.totals.api_equivalent.micro_usd, 3_600_000);
        assert_eq!(page.totals.api_equivalent.unpriced_tokens, 1_100_000);
        assert_eq!(page.models[0].totals.api_equivalent.micro_usd, 3_600_000);
        let event_value = |request_id: &str| {
            page.events
                .iter()
                .find(|event| event.request_id == request_id)
                .unwrap()
                .api_equivalent
        };
        assert_eq!(event_value("req_source_cheap").micro_usd, 1_200_000);
        assert_eq!(event_value("req_source_expensive").micro_usd, 2_400_000);
        assert_eq!(event_value("req_account").unpriced_tokens, 1_100_000);

        let equivalents = database
            .api_equivalents_with_price_overrides(&BTreeMap::new(), &source_prices)
            .unwrap();
        assert_eq!(equivalents.sources["source_cheap"].micro_usd, 1_200_000);
        assert_eq!(equivalents.sources["source_expensive"].micro_usd, 2_400_000);
        assert_eq!(equivalents.accounts["account_1"].micro_usd, 0);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_v1_migrates_existing_rows_to_attempt_one() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-v1-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_001).unwrap();
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, local_key_id, source_id, wire_api, success, http_status, latency_ms
                ) VALUES ('req_old', 'key_1', 'source_1', 'responses', 1, 200, 3)",
                [],
            )
            .unwrap();
        drop(connection);

        let database = TelemetryDb::open(&path).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 1);
        let version: u32 = database
            .connection
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_v14_migration_keeps_the_latest_attempt_per_request() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-v14-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        let connection = Connection::open(&path).unwrap();
        for migration in [
            MIGRATION_001,
            MIGRATION_002,
            MIGRATION_003,
            MIGRATION_004,
            MIGRATION_005,
            MIGRATION_006,
            MIGRATION_007,
            MIGRATION_008,
            MIGRATION_009,
            MIGRATION_010,
            MIGRATION_011,
            MIGRATION_012,
            MIGRATION_013,
            MIGRATION_014,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms
                ) VALUES ('req_duplicate', 1, 'key', 'source_1', 'responses', 0, 503, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms
                ) VALUES ('req_duplicate', 2, 'key', 'source_2', 'responses', 1, 200, 2)",
                [],
            )
            .unwrap();
        drop(connection);

        let database = TelemetryDb::open(&path).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 2);
        assert!(logs[0].success);
        assert_eq!(logs[0].source_id, "source_2");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_schema_has_no_secret_or_body_columns() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-schema-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let connection = database.connection.lock().unwrap();
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info('request_logs')")
            .unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(!columns.iter().any(|column| {
            let column = column.to_lowercase();
            column.contains("secret")
                || column.contains("prompt")
                || column.contains("request_body")
                || column.contains("response_body")
        }));
        drop(statement);
        drop(connection);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn newer_usage_schema_is_rejected_without_rewrite() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-future-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        let connection = Connection::open(&path).unwrap();
        let future_version = LOCAL_DATABASE_SCHEMA_VERSION + 1;
        connection
            .pragma_update(None, "user_version", future_version)
            .unwrap();
        drop(connection);

        assert!(matches!(
            TelemetryDb::open(&path).err().unwrap().code,
            ErrorCode::UnsupportedSchema
        ));
        let version: u32 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, future_version);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_retention_prunes_old_rows_on_open_and_periodically() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-retention-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        drop(TelemetryDb::open(&path).unwrap());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                    requested_model, resolved_model, wire_api, success, http_status, latency_ms,
                    input_tokens, cached_input_tokens, output_tokens, total_tokens, created_at
                ) VALUES ('old-open', 1, 'key', 'source', 'account', 'account',
                    'gpt-5.4', 'gpt-5.4', 'responses', 1, 200, 1, 20, 10, 8, 28,
                    datetime('now', '-31 days'))",
                [],
            )
            .unwrap();
        drop(connection);

        let database = TelemetryDb::open(&path).unwrap();
        assert!(database.list(10).unwrap().is_empty());
        assert_eq!(
            database.api_equivalents().unwrap().accounts.get("account"),
            Some(&ApiEquivalentSummary {
                micro_usd: 148,
                priced_tokens: 28,
                unpriced_tokens: 0,
            })
        );
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO request_logs (
                    id, request_id, attempt, local_key_id, source_id, candidate_id,
                    requested_model, resolved_model, wire_api, success, http_status, latency_ms,
                    input_tokens, cached_input_tokens, output_tokens, total_tokens, created_at
                ) VALUES (255, 'old-trigger', 1, 'key', 'source', 'source',
                    'gpt-5.4', 'gpt-5.4', 'responses', 1, 200, 1, 20, 10, 8, 28,
                    datetime('now', '-31 days'))",
                [],
            )
            .unwrap();
        database
            .record(&UsageEvent {
                request_id: "trigger-256".into(),
                attempt: 1,
                local_key_id: "key".into(),
                source_id: "source".into(),
                candidate_id: None,
                account_id: None,
                routing: None,
                requested_model: None,
                resolved_model: None,
                wire_api: WireApi::Responses,
                service_tier: DefaultServiceTier::Standard,
                applied_service_tier: None,
                success: true,
                http_status: 200,
                error_category: None,
                tool_use: ToolUseDiagnostics::default(),
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: None,
                latency_ms: 1,
                ttft_ms: None,
                generation_ms: None,
                input_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_tokens: None,
                output_tokens: None,
                total_tokens: None,
                quota_snapshot: None,
            })
            .unwrap();
        assert_eq!(database.list(10).unwrap().len(), 1);
        assert_eq!(
            database.api_equivalents().unwrap().sources.get("source"),
            Some(&ApiEquivalentSummary {
                micro_usd: 148,
                priced_tokens: 28,
                unpriced_tokens: 0,
            })
        );
        database.clear().unwrap();
        let equivalents = database.api_equivalents().unwrap();
        assert!(equivalents.accounts.is_empty());
        assert!(equivalents.sources.is_empty());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}

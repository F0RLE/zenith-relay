use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};
use serde::Serialize;
use std::{collections::HashMap, path::Path, sync::Mutex};
use zenith_relay_core::{
    estimate_api_equivalent,
    protocol::{UsageGroup, UsageQuery, UsageTotals},
    ApiEquivalentSummary, ResponseAffinityBinding, RoutingDiagnostics, UsageEvent, WireApi,
};

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

const USAGE_SCHEMA_VERSION: u32 = 10;
const PRUNE_USAGE_SQL: &str =
    "DELETE FROM request_logs WHERE created_at < datetime('now', '-30 days')";

pub struct TelemetryDb {
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLog {
    pub id: i64,
    pub created_at: String,
    pub request_id: String,
    pub attempt: u16,
    pub local_key_id: String,
    pub source_id: String,
    pub candidate_id: Option<String>,
    pub account_id: Option<String>,
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: String,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
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
}

#[derive(Default)]
pub struct UsageEquivalents {
    pub accounts: HashMap<String, ApiEquivalentSummary>,
    pub sources: HashMap<String, ApiEquivalentSummary>,
}

impl TelemetryDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_error)?;
        if version > USAGE_SCHEMA_VERSION {
            return Err(LocalPoolError::new(
                ErrorCode::UnsupportedSchema,
                format!(
                    "usage database schema {version} is newer than supported schema {USAGE_SCHEMA_VERSION}"
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
        connection.execute(PRUNE_USAGE_SQL, []).map_err(db_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
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
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "INSERT OR IGNORE INTO request_logs (
                    request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                    requested_model, resolved_model, wire_api, success, http_status,
                    error_category, latency_ms, ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens, routing_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
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
                    routing_json,
                ],
            )
            .map(|_| ())
            .map_err(db_error)
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
                    ttft_ms, generation_ms, input_tokens, cached_input_tokens, cache_write_input_tokens,
                    reasoning_tokens, output_tokens, total_tokens, routing_json
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

    pub fn usage_page(&self, query: &UsageQuery) -> Result<LocalUsagePage> {
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
        for group in &mut models {
            group.totals.api_equivalent = estimate_api_equivalent(
                (!group.key.is_empty()).then_some(group.key.as_str()),
                Some(group.totals.input_tokens),
                (group.totals.cached_input_samples > 0).then_some(group.totals.cached_input_tokens),
                (group.totals.cache_write_input_samples > 0)
                    .then_some(group.totals.cache_write_input_tokens),
                Some(group.totals.output_tokens),
                Some(group.totals.total_tokens),
            );
            totals.api_equivalent.merge(group.totals.api_equivalent);
        }
        let pool_members = usage_groups(
            &connection,
            &where_sql,
            &values,
            "COALESCE(account_id, source_id, '')",
        )?;
        let total = totals.requests;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let sql = format!(
            "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                local_key_id, source_id, candidate_id, account_id, requested_model,
                resolved_model, wire_api, success, http_status, error_category, latency_ms,
                ttft_ms, generation_ms, input_tokens, cached_input_tokens, cache_write_input_tokens,
                reasoning_tokens, output_tokens, total_tokens, routing_json
             FROM request_logs{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let mut page_values = values;
        page_values.push(SqlValue::Integer(i64::from(page_size)));
        page_values.push(SqlValue::Integer(offset.min(i64::MAX as u64) as i64));
        let events = statement
            .query_map(params_from_iter(page_values.iter()), usage_log_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
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
        })
    }

    pub fn api_equivalents(&self) -> Result<UsageEquivalents> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model),
                    SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
                    SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens),
                    COUNT(cached_input_tokens), COUNT(cache_write_input_tokens)
                 FROM request_logs
                 GROUP BY 1, 2, 3",
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
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    estimate_api_equivalent(
                        row.get::<_, Option<String>>(2)?.as_deref(),
                        input_tokens.map(rust_u64),
                        (input_samples > 0 && cached_samples == input_samples)
                            .then(|| cached_input_tokens.map(rust_u64))
                            .flatten(),
                        (input_samples > 0 && cache_write_samples == input_samples)
                            .then(|| cache_write_input_tokens.map(rust_u64))
                            .flatten(),
                        output_tokens.map(rust_u64),
                        total_tokens.map(rust_u64),
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
        Ok(equivalents)
    }

    pub fn clear(&self) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute("DELETE FROM request_logs", [])
            .map(|_| ())
            .map_err(db_error)
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
                 FROM response_affinity ORDER BY updated_at_ms DESC",
            )
            .map_err(db_error)?;
        let bindings = statement
            .query_map([], affinity_binding_from_row)
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
    COALESCE(SUM(generation_ms), 0), COUNT(generation_ms), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms IS NOT NULL \
        THEN COALESCE(output_tokens, 0) ELSE 0 END), 0), \
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0), \
    COUNT(cached_input_tokens), COALESCE(SUM(cache_write_input_tokens), 0), \
    COUNT(cache_write_input_tokens), COALESCE(SUM(reasoning_tokens), 0), \
    COALESCE(SUM(output_tokens), 0), \
    COALESCE(SUM(total_tokens), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 \
        THEN output_tokens ELSE 0 END), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 \
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
    if let Some(value) = query.local_key_query.as_deref() {
        clauses.push("local_key_id LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
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
    let routing_json: Option<String> = row.get(23)?;
    Ok(UsageLog {
        id: row.get(0)?,
        created_at: row.get(1)?,
        request_id: row.get(2)?,
        attempt: row.get(3)?,
        local_key_id: row.get(4)?,
        source_id: row.get(5)?,
        candidate_id: row.get(6)?,
        account_id: row.get(7)?,
        routing: routing_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        requested_model: row.get(8)?,
        resolved_model: row.get(9)?,
        wire_api: normalize_wire_api(row.get(10)?),
        success: row.get(11)?,
        http_status: row.get(12)?,
        error_category: row.get(13)?,
        latency_ms: rust_u64(latency_ms),
        ttft_ms: ttft_ms.map(rust_u64),
        generation_ms: generation_ms.map(rust_u64),
        input_tokens: input_tokens.map(rust_u64),
        cached_input_tokens: cached_input_tokens.map(rust_u64),
        cache_write_input_tokens: cache_write_input_tokens.map(rust_u64),
        reasoning_tokens: reasoning_tokens.map(rust_u64),
        output_tokens: output_tokens.map(rust_u64),
        total_tokens: total_tokens.map(rust_u64),
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

fn db_error(error: rusqlite::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, format!("usage database error: {error}"))
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{SelectionReason, WireApi};

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
            candidate_id: Some("source_1".into()),
            account_id: None,
            routing: Some(RoutingDiagnostics {
                reason: SelectionReason::QuotaHeadroom,
                eligible_candidates: 4,
                quota_remaining_basis_points: Some(6_300),
                effective_weight: 6_300,
                in_flight_before: 0,
                dispatches_before: 3,
            }),
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            success: true,
            http_status: 200,
            error_category: None,
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
        };
        TelemetryDb::open(&path).unwrap().record(&event).unwrap();
        let database = TelemetryDb::open(&path).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].created_at.ends_with('Z'));
        assert_eq!(logs[0].candidate_id.as_deref(), Some("source_1"));
        assert_eq!(logs[0].ttft_ms, Some(4));
        assert_eq!(logs[0].cached_input_tokens, Some(1));
        assert_eq!(logs[0].cache_write_input_tokens, Some(1));
        assert_eq!(logs[0].reasoning_tokens, Some(2));
        assert_eq!(
            logs[0].routing.as_ref().map(|routing| routing.reason),
            Some(SelectionReason::QuotaHeadroom)
        );
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
            success: true,
            http_status: 200,
            error_category: None,
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 428,
            ttft_ms: Some(128),
            generation_ms: Some(300),
            input_tokens: Some(20),
            cached_input_tokens: Some(12),
            cache_write_input_tokens: Some(4),
            reasoning_tokens: Some(5),
            output_tokens: Some(8),
            total_tokens: Some(28),
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
        event.cache_write_input_tokens = Some(0);
        event.reasoning_tokens = Some(0);
        event.output_tokens = Some(20);
        event.total_tokens = Some(30);
        database.record(&event).unwrap();

        let page = database
            .usage_page(&UsageQuery {
                page: 1,
                page_size: 1,
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.total, 2);
        assert_eq!(page.total_pages, 2);
        assert_eq!(page.totals.requests, 2);
        assert_eq!(page.totals.total_tokens, 58);
        assert_eq!(page.totals.cache_write_input_tokens, 4);
        assert_eq!(page.totals.cache_write_input_samples, 2);
        assert_eq!(page.totals.speed_output_tokens, 28);
        assert_eq!(page.totals.speed_duration_ms, 928);
        assert_eq!(page.totals.api_equivalent.priced_tokens, 58);
        assert_eq!(page.models.len(), 1);
        assert_eq!(page.pool_members.len(), 2);
        assert_eq!(page.events[0].wire_api, "chat_completions");

        let chat = database
            .usage_page(&UsageQuery {
                wire_api: Some(WireApi::ChatCompletions),
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(chat.total, 1);
        assert_eq!(chat.totals.cache_write_input_samples, 1);
        assert_eq!(chat.events[0].request_id, "req_page_2");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_keeps_each_fallback_attempt() {
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
            success: false,
            http_status: 503,
            error_category: Some("upstream_unavailable".into()),
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
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].attempt, 2);
        assert_eq!(logs[1].attempt, 1);
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
            success: true,
            http_status: 200,
            error_category: None,
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 12,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(20),
            cached_input_tokens: Some(10),
            cache_write_input_tokens: Some(5),
            reasoning_tokens: Some(3),
            output_tokens: Some(8),
            total_tokens: Some(28),
        };
        database.record(&event).unwrap();
        let equivalents = database.api_equivalents().unwrap();
        assert_eq!(
            equivalents.accounts.get("account_1"),
            Some(&ApiEquivalentSummary {
                micro_usd: 149,
                priced_tokens: 28,
                unpriced_tokens: 0,
            })
        );
        assert!(equivalents.sources.is_empty());
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
        assert_eq!(version, USAGE_SCHEMA_VERSION);
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
        let future_version = USAGE_SCHEMA_VERSION + 1;
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
                    request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms, created_at
                ) VALUES ('old-open', 1, 'key', 'source', 'responses', 1, 200, 1,
                    datetime('now', '-31 days'))",
                [],
            )
            .unwrap();
        drop(connection);

        let database = TelemetryDb::open(&path).unwrap();
        assert!(database.list(10).unwrap().is_empty());
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO request_logs (
                    id, request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms, created_at
                ) VALUES (255, 'old-trigger', 1, 'key', 'source', 'responses', 1, 200, 1,
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
                success: true,
                http_status: 200,
                error_category: None,
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
            })
            .unwrap();
        assert_eq!(database.list(10).unwrap().len(), 1);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}

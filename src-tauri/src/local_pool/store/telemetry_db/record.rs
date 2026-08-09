use super::{
    db_error, sql_u64, DefaultServiceTier, ErrorCode, LocalPoolError, Result, TelemetryDb,
    UsageEvent, ARCHIVE_USAGE_SQL,
};
use rusqlite::params;

impl TelemetryDb {
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
                    event.wire_api.as_str(),
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
                    event.service_tier.as_str(),
                    event.applied_service_tier.map(DefaultServiceTier::as_str),
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
}

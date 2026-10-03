use super::usage::{usage_totals_from_event, usage_totals_from_sample, UsageTotalsSample};
use super::{
    db_error, sql_u64, ErrorCode, LocalPoolError, Result, TelemetryDb, UsageEvent,
    ARCHIVE_USAGE_SQL,
};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::atomic::Ordering;

mod aggregate;
use aggregate::{apply_aggregate_delta, UsageAggregate};

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
        let upstream_error_json = event
            .upstream_error
            .as_ref()
            .filter(|_| !event.success)
            .map(|details| serde_json::to_string(&details.sanitized()))
            .transpose()
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::Io,
                    "upstream error diagnostics serialization failed",
                )
            })?;
        let requested_reasoning_effort = event
            .requested_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort);
        let effective_reasoning_effort = event
            .effective_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort);
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let previous = previous_usage_log(&transaction, &event.request_id)?;
        let accepted = previous
            .as_ref()
            .is_none_or(|previous| i64::from(event.attempt) >= previous.attempt);
        let changed = accepted
            && transaction
                .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                    requested_model, resolved_model, wire_api, success, http_status,
                    error_category, latency_ms, ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json, error_origin,
                    requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl,
                    usage_aggregate_recorded, client_context_id, upstream_error_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, 1, ?30, ?31)
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
                    tool_use_json = excluded.tool_use_json,
                    error_origin = excluded.error_origin,
                    requested_reasoning_effort = excluded.requested_reasoning_effort,
                    effective_reasoning_effort = excluded.effective_reasoning_effort,
                    cache_write_ttl = excluded.cache_write_ttl,
                    client_context_id = excluded.client_context_id,
                    upstream_error_json = excluded.upstream_error_json,
                    usage_aggregate_recorded = 1
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
                    event.applied_service_tier.as_deref(),
                    routing_json,
                    tool_use_json,
                    event.error_origin().map(|origin| origin.as_str()),
                    requested_reasoning_effort,
                    effective_reasoning_effort,
                    event.cache_write_ttl.as_deref().and_then(
                        zenith_relay_core::usage::normalize_reported_cache_ttls,
                    ),
                    event.client_context_id,
                    upstream_error_json,
                ],
            )
            .map_err(db_error)?
            > 0;
        if changed {
            if let Some(previous) = previous.as_ref().filter(|previous| previous.aggregated) {
                apply_aggregate_delta(&transaction, &previous.aggregate, -1)?;
            }
            apply_aggregate_delta(&transaction, &UsageAggregate::from_event(event), 1)?;
        }
        // `last_insert_rowid` is unchanged by an upsert that takes the
        // conflict-update path. Only run retention after a new request row;
        // otherwise replacing a request whose old id is divisible by 256
        // would rescan and rewrite the usage database repeatedly.
        let archived = previous.is_none() && changed && transaction.last_insert_rowid() % 256 == 0;
        if archived {
            transaction
                .execute_batch(ARCHIVE_USAGE_SQL)
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        if archived {
            self.clear_cached_usage_totals()?;
        } else if changed {
            let previous_totals = previous
                .as_ref()
                .map(|previous| usage_totals_from_sample(previous.totals));
            self.update_cached_usage_totals(previous_totals, usage_totals_from_event(event))?;
        }
        drop(connection);
        if changed || archived {
            self.invalidate_usage_cache();
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        self.lock_connection()?
            .execute_batch("DELETE FROM request_logs; DELETE FROM usage_candidate_rollups;")
            .map_err(db_error)?;
        self.clear_cached_usage_totals()?;
        self.invalidate_usage_cache();
        Ok(())
    }

    pub(super) fn invalidate_usage_cache(&self) {
        self.usage_revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cached) = self.api_equivalent_cache.lock() {
            *cached = None;
        }
        if let Ok(mut cached) = self.quota_equivalent_cache.lock() {
            *cached = None;
        }
    }
}

struct PreviousUsageLog {
    attempt: i64,
    aggregated: bool,
    aggregate: UsageAggregate,
    totals: UsageTotalsSample,
}

fn previous_usage_log(
    transaction: &Transaction<'_>,
    request_id: &str,
) -> Result<Option<PreviousUsageLog>> {
    transaction
        .query_row(
            "SELECT attempt,
                usage_aggregate_recorded,
                CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                input_tokens, cached_input_tokens, cache_write_input_tokens, cache_write_ttl,
                output_tokens, total_tokens, success, latency_ms, ttft_ms, generation_ms,
                reasoning_tokens
             FROM request_logs WHERE request_id = ?1",
            [request_id],
            |row| {
                Ok(PreviousUsageLog {
                    attempt: row.get(0)?,
                    aggregated: row.get(1)?,
                    aggregate: UsageAggregate::from_row(row, 2)?,
                    totals: UsageTotalsSample {
                        success: row.get(11)?,
                        latency_ms: non_negative_i64(row.get(12)?),
                        ttft_ms: row.get::<_, Option<i64>>(13)?.map(non_negative_i64),
                        generation_ms: row.get::<_, Option<i64>>(14)?.map(non_negative_i64),
                        reasoning_tokens: row.get::<_, Option<i64>>(15)?.map(non_negative_i64),
                        input_tokens: row.get::<_, Option<i64>>(5)?.map(non_negative_i64),
                        cached_input_tokens: row.get::<_, Option<i64>>(6)?.map(non_negative_i64),
                        cache_write_input_tokens: row
                            .get::<_, Option<i64>>(7)?
                            .map(non_negative_i64),
                        output_tokens: row.get::<_, Option<i64>>(9)?.map(non_negative_i64),
                        total_tokens: row.get::<_, Option<i64>>(10)?.map(non_negative_i64),
                    },
                })
            },
        )
        .optional()
        .map_err(db_error)
}

fn non_negative_i64(value: i64) -> u64 {
    value.max(0) as u64
}

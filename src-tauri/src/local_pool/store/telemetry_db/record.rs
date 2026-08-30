use super::{
    db_error, sql_u64, ErrorCode, LocalPoolError, Result, TelemetryDb, UsageEvent,
    ARCHIVE_USAGE_SQL,
};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::atomic::Ordering;

#[derive(Clone, Default)]
struct UsageAggregate {
    candidate_kind: String,
    candidate_id: String,
    model: String,
    input_tokens: i64,
    input_samples: i64,
    cached_input_tokens: i64,
    cached_input_samples: i64,
    cache_write_input_tokens: i64,
    cache_write_input_samples: i64,
    cache_write_5m_tokens: i64,
    cache_write_1h_tokens: i64,
    unknown_cache_write_tokens: i64,
    output_tokens: i64,
    output_samples: i64,
    total_tokens: i64,
    total_samples: i64,
}

impl UsageAggregate {
    fn from_event(event: &UsageEvent) -> Self {
        Self::from_values(
            if event.account_id.is_some() {
                "account"
            } else {
                "source"
            },
            event
                .account_id
                .as_deref()
                .unwrap_or(event.source_id.as_str()),
            event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref())
                .unwrap_or_default(),
            event.input_tokens,
            event.cached_input_tokens,
            event.cache_write_input_tokens,
            event
                .cache_write_ttl
                .and_then(zenith_relay_core::CacheWriteTtl::anthropic_ttl),
            event.output_tokens,
            event.total_tokens,
        )
    }

    fn from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        let candidate_kind: String = row.get(offset)?;
        let candidate_id: String = row.get(offset + 1)?;
        let model: String = row.get(offset + 2)?;
        let input_tokens: Option<i64> = row.get(offset + 3)?;
        let cached_input_tokens: Option<i64> = row.get(offset + 4)?;
        let cache_write_input_tokens: Option<i64> = row.get(offset + 5)?;
        let cache_write_ttl: Option<String> = row.get(offset + 6)?;
        let output_tokens: Option<i64> = row.get(offset + 7)?;
        let total_tokens: Option<i64> = row.get(offset + 8)?;
        Ok(Self::from_values(
            &candidate_kind,
            &candidate_id,
            &model,
            input_tokens.map(|value| value.max(0) as u64),
            cached_input_tokens.map(|value| value.max(0) as u64),
            cache_write_input_tokens.map(|value| value.max(0) as u64),
            cache_write_ttl.as_deref(),
            output_tokens.map(|value| value.max(0) as u64),
            total_tokens.map(|value| value.max(0) as u64),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn from_values(
        candidate_kind: &str,
        candidate_id: &str,
        model: &str,
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_write_input_tokens: Option<u64>,
        cache_write_ttl: Option<&str>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
    ) -> Self {
        let mut aggregate = Self {
            candidate_kind: candidate_kind.to_string(),
            candidate_id: candidate_id.to_string(),
            model: model.to_string(),
            input_tokens: input_tokens.map(sql_u64).unwrap_or_default(),
            input_samples: i64::from(input_tokens.is_some()),
            cached_input_tokens: cached_input_tokens.map(sql_u64).unwrap_or_default(),
            cached_input_samples: i64::from(cached_input_tokens.is_some()),
            cache_write_input_tokens: cache_write_input_tokens.map(sql_u64).unwrap_or_default(),
            cache_write_input_samples: i64::from(cache_write_input_tokens.is_some()),
            output_tokens: output_tokens.map(sql_u64).unwrap_or_default(),
            output_samples: i64::from(output_tokens.is_some()),
            total_tokens: total_tokens.map(sql_u64).unwrap_or_default(),
            total_samples: i64::from(total_tokens.is_some()),
            ..Self::default()
        };
        let cache_write_tokens = cache_write_input_tokens.map(sql_u64).unwrap_or_default();
        match cache_write_ttl {
            Some("5m") => aggregate.cache_write_5m_tokens = cache_write_tokens,
            Some("1h") => aggregate.cache_write_1h_tokens = cache_write_tokens,
            _ => aggregate.unknown_cache_write_tokens = cache_write_tokens,
        }
        aggregate
    }
}

fn apply_aggregate_delta(
    transaction: &Transaction<'_>,
    aggregate: &UsageAggregate,
    multiplier: i64,
) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO usage_candidate_rollups(
                candidate_kind, candidate_id, model,
                input_tokens, input_samples, cached_input_tokens, cached_input_samples,
                cache_write_input_tokens, cache_write_input_samples,
                cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
                output_tokens, output_samples, total_tokens, total_samples
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
                total_samples = total_samples + excluded.total_samples",
            params![
                &aggregate.candidate_kind,
                &aggregate.candidate_id,
                &aggregate.model,
                aggregate.input_tokens * multiplier,
                aggregate.input_samples * multiplier,
                aggregate.cached_input_tokens * multiplier,
                aggregate.cached_input_samples * multiplier,
                aggregate.cache_write_input_tokens * multiplier,
                aggregate.cache_write_input_samples * multiplier,
                aggregate.cache_write_5m_tokens * multiplier,
                aggregate.cache_write_1h_tokens * multiplier,
                aggregate.unknown_cache_write_tokens * multiplier,
                aggregate.output_tokens * multiplier,
                aggregate.output_samples * multiplier,
                aggregate.total_tokens * multiplier,
                aggregate.total_samples * multiplier,
            ],
        )
        .map_err(db_error)?;
    Ok(())
}

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
        let requested_reasoning_effort = event
            .requested_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort);
        let effective_reasoning_effort = event
            .effective_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort);
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let previous = transaction
            .query_row(
                "SELECT attempt,
                    usage_aggregate_recorded,
                    CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                    input_tokens, cached_input_tokens, cache_write_input_tokens, cache_write_ttl,
                    output_tokens, total_tokens
                 FROM request_logs WHERE request_id = ?1",
                [event.request_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, bool>(1)?,
                        UsageAggregate::from_row(row, 2)?,
                    ))
                },
            )
            .optional()
            .map_err(db_error)?;
        let accepted = previous
            .as_ref()
            .is_none_or(|(attempt, _, _)| i64::from(event.attempt) >= *attempt);
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
                    usage_aggregate_recorded, client_context_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, 1, ?30)
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
                    event.cache_write_ttl.and_then(zenith_relay_core::CacheWriteTtl::anthropic_ttl),
                    event.client_context_id,
                ],
            )
            .map_err(db_error)?
            > 0;
        if changed {
            if let Some((_, _was_aggregated, previous)) =
                previous.as_ref().filter(|(_, aggregated, _)| *aggregated)
            {
                apply_aggregate_delta(&transaction, previous, -1)?;
            }
            apply_aggregate_delta(&transaction, &UsageAggregate::from_event(event), 1)?;
        }
        if transaction.last_insert_rowid() % 256 == 0 {
            transaction
                .execute_batch(ARCHIVE_USAGE_SQL)
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        drop(connection);
        if changed {
            self.invalidate_usage_cache();
        }
        Ok(())
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

    pub(super) fn invalidate_usage_cache(&self) {
        self.usage_revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cached) = self.api_equivalent_cache.lock() {
            *cached = None;
        }
    }
}

use super::super::sqlite::{db_error, to_json, Store};
use rusqlite::{params, TransactionBehavior};
use sha2::{Digest, Sha256};
use zenith_relay_core::UsageEvent;

impl Store {
    pub fn record_usage(&self, event: &UsageEvent, created_at_ms: u64) -> Result<(), String> {
        self.record_usage_batch(&[(event, created_at_ms)])
    }

    pub fn record_usage_batch(&self, events: &[(&UsageEvent, u64)]) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        {
            let mut statement = transaction
                .prepare(
                    r#"INSERT INTO usage_events(
                        request_id, attempt, local_key_id, candidate_kind, candidate_hint,
                        requested_model, resolved_model, wire_api, transport, success, http_status,
                        error_category, latency_ms, ttft_ms, generation_ms, input_tokens,
                        cached_input_tokens, cache_write_input_tokens, reasoning_tokens,
                        output_tokens, total_tokens, created_at_ms, routing_json,
                        service_tier, applied_service_tier, tool_use_json, error_origin,
                        requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl, upstream_error_json
                    ) SELECT
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23,
                        ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?32
                    WHERE NOT EXISTS (
                        SELECT 1 FROM usage_request_tombstones WHERE request_id = ?1
                    )
                    AND (?4 != 'account' OR EXISTS (
                        SELECT 1 FROM accounts WHERE id = ?31
                    ))
                    ON CONFLICT(request_id) DO UPDATE SET
                        attempt=excluded.attempt,
                        local_key_id=excluded.local_key_id,
                        candidate_kind=excluded.candidate_kind,
                        candidate_hint=excluded.candidate_hint,
                        requested_model=excluded.requested_model,
                        resolved_model=excluded.resolved_model,
                        wire_api=excluded.wire_api,
                        transport=excluded.transport,
                        success=excluded.success,
                        http_status=excluded.http_status,
                        error_category=excluded.error_category,
                        latency_ms=excluded.latency_ms,
                        ttft_ms=excluded.ttft_ms,
                        generation_ms=excluded.generation_ms,
                        input_tokens=excluded.input_tokens,
                        cached_input_tokens=excluded.cached_input_tokens,
                        cache_write_input_tokens=excluded.cache_write_input_tokens,
                        reasoning_tokens=excluded.reasoning_tokens,
                        output_tokens=excluded.output_tokens,
                        total_tokens=excluded.total_tokens,
                        created_at_ms=excluded.created_at_ms,
                        routing_json=excluded.routing_json,
                        service_tier=excluded.service_tier,
                        applied_service_tier=excluded.applied_service_tier,
                        tool_use_json=excluded.tool_use_json,
                        error_origin=excluded.error_origin,
                        requested_reasoning_effort=excluded.requested_reasoning_effort,
                        effective_reasoning_effort=excluded.effective_reasoning_effort,
                        cache_write_ttl=excluded.cache_write_ttl,
                        upstream_error_json=excluded.upstream_error_json
                    WHERE excluded.attempt >= usage_events.attempt"#,
                )
                .map_err(db_error)?;
            for (event, created_at_ms) in events {
                let candidate_id = event
                    .account_id
                    .as_deref()
                    .or(event.candidate_id.as_deref())
                    .unwrap_or(&event.source_id);
                let candidate_kind = if event.account_id.is_some() {
                    "account"
                } else {
                    "source"
                };
                let candidate_hint =
                    hex::encode(Sha256::digest(candidate_id.as_bytes()))[..12].to_string();
                let routing_json = event.routing.as_ref().map(to_json).transpose()?;
                let tool_use_json = event
                    .tool_use
                    .has_evidence()
                    .then(|| to_json(&event.tool_use))
                    .transpose()?;
                statement
                    .execute(params![
                        event.request_id,
                        event.attempt,
                        event.local_key_id,
                        candidate_kind,
                        candidate_hint,
                        event.requested_model,
                        event.resolved_model,
                        event.wire_api.as_str(),
                        event.transport.as_str(),
                        i64::from(event.success),
                        i64::from(event.http_status),
                        event.error_category,
                        event.latency_ms as i64,
                        event.ttft_ms.map(|value| value as i64),
                        event.generation_ms.map(|value| value as i64),
                        event.input_tokens.map(|value| value as i64),
                        event.cached_input_tokens.map(|value| value as i64),
                        event.cache_write_input_tokens.map(|value| value as i64),
                        event.reasoning_tokens.map(|value| value as i64),
                        event.output_tokens.map(|value| value as i64),
                        event.total_tokens.map(|value| value as i64),
                        *created_at_ms as i64,
                        routing_json,
                        event.service_tier.as_str(),
                        event.applied_service_tier.as_deref(),
                        tool_use_json,
                        event.error_origin().map(|origin| origin.as_str()),
                        event
                            .requested_reasoning_effort
                            .as_deref()
                            .and_then(zenith_relay_core::normalize_reasoning_effort),
                        event
                            .effective_reasoning_effort
                            .as_deref()
                            .and_then(zenith_relay_core::normalize_reasoning_effort),
                        event
                            .cache_write_ttl
                            .as_deref()
                            .and_then(zenith_relay_core::usage::normalize_reported_cache_ttls,),
                        candidate_id,
                        event
                            .upstream_error
                            .as_ref()
                            .filter(|_| !event.success)
                            .map(|details| to_json(&details.sanitized()))
                            .transpose()?,
                    ])
                    .map_err(db_error)?;
            }
        }
        transaction.commit().map_err(db_error)
    }
}

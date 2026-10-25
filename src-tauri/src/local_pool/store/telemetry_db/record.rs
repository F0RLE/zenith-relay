use super::usage::{
    apply_usage_totals_delta, consumption_totals, request_totals, usage_totals_from_event,
    usage_totals_from_sample, UsageTotalsSample,
};
use super::{
    db_error, lock_error, sql_u64, ErrorCode, LocalPoolError, Result, TelemetryDb, UsageEvent,
    ARCHIVE_USAGE_SQL,
};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::{atomic::Ordering, LazyLock};

mod aggregate;
pub(super) use aggregate::{apply_aggregate_delta, UsageAggregate};
#[cfg(test)]
mod tests;

enum UsageRecordKind {
    FinalRequest,
    AttemptHistory,
}

impl UsageRecordKind {
    fn upsert_sql(&self) -> &'static str {
        static REQUEST: LazyLock<String> =
            LazyLock::new(|| UsageRecordKind::FinalRequest.build_upsert_sql());
        static ATTEMPT: LazyLock<String> =
            LazyLock::new(|| UsageRecordKind::AttemptHistory.build_upsert_sql());
        match self {
            Self::FinalRequest => &REQUEST,
            Self::AttemptHistory => &ATTEMPT,
        }
    }

    fn previous_sql(&self) -> &'static str {
        const COLUMNS: &str = "SELECT attempt,
            usage_aggregate_recorded,
            CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
            COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
            input_tokens, cached_input_tokens, cache_write_input_tokens, cache_write_ttl,
            output_tokens, total_tokens, applied_service_tier, success, latency_ms, ttft_ms,
            generation_ms, reasoning_tokens";
        static REQUEST: LazyLock<String> =
            LazyLock::new(|| format!("{COLUMNS} FROM request_logs WHERE request_id = ?1"));
        static ATTEMPT: LazyLock<String> = LazyLock::new(|| {
            format!("{COLUMNS} FROM request_attempts WHERE request_id = ?1 AND attempt = ?2")
        });
        match self {
            Self::FinalRequest => &REQUEST,
            Self::AttemptHistory => &ATTEMPT,
        }
    }

    fn build_upsert_sql(&self) -> String {
        let (table_name, conflict_columns) = match self {
            Self::FinalRequest => ("request_logs", "request_id"),
            Self::AttemptHistory => ("request_attempts", "request_id, attempt"),
        };
        format!(
            "INSERT INTO {table_name} (
                request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                requested_model, resolved_model, wire_api, transport, success, http_status,
                error_category, latency_ms, ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                service_tier, applied_service_tier, routing_json, tool_use_json, error_origin,
                requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl,
                usage_aggregate_recorded, client_context_id, upstream_error_json, image_usage_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, 1, ?31, ?32, ?33)
            ON CONFLICT({conflict_columns}) DO UPDATE SET
                created_at = CURRENT_TIMESTAMP,
                attempt = excluded.attempt,
                local_key_id = excluded.local_key_id,
                source_id = excluded.source_id,
                candidate_id = excluded.candidate_id,
                account_id = excluded.account_id,
                requested_model = excluded.requested_model,
                resolved_model = excluded.resolved_model,
                wire_api = excluded.wire_api,
                transport = excluded.transport,
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
                image_usage_json = excluded.image_usage_json,
                usage_aggregate_recorded = 1
            WHERE excluded.attempt >= {table_name}.attempt
              AND (excluded.attempt > {table_name}.attempt
                OR COALESCE(json_extract({table_name}.routing_json, '$.requestTiming.projectionOnly'), 0) = 0
                OR COALESCE(json_extract(excluded.routing_json, '$.requestTiming.projectionOnly'), 0) = 1)"
        )
    }
}

impl TelemetryDb {
    pub fn record(&self, event: &UsageEvent) -> Result<()> {
        if event.attempt == 0 && !is_local_pre_dispatch_rejection(event) {
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
        let image_usage_json = event
            .image_usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "image usage serialization failed"))?;
        let effective_reasoning_effort = event
            .effective_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort);
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let previous_usage = previous_usage_log(&transaction, &event.request_id, None)?;
        let projection_only = event.is_request_completion();
        // Attempt zero describes a locally rejected client request, not a
        // provider send. Keep its final outcome without inventing consumption.
        let record_send = event.attempt > 0 && !projection_only;
        // A completion can only close an existing request. It is not a new send
        // and cannot recreate telemetry that the user cleared in the meantime.
        if projection_only && previous_usage.is_none() {
            return Ok(());
        }
        let request_timing = event
            .routing
            .as_ref()
            .and_then(|routing| routing.request_timing.as_ref());
        let request_latency_ms =
            request_timing.map_or(event.latency_ms, |timing| timing.duration_ms);
        let request_ttft_ms = request_timing.map_or(event.ttft_ms, |timing| timing.ttft_ms);
        let sql_request_latency_ms = sql_u64(request_latency_ms);
        let sql_request_ttft_ms = request_ttft_ms.map(sql_u64);
        let previous_attempt = if record_send {
            previous_usage_log(&transaction, &event.request_id, Some(event.attempt))?.or_else(
                || {
                    previous_usage
                        .as_ref()
                        .filter(|old| old.attempt == i64::from(event.attempt))
                        .cloned()
                },
            )
        } else {
            None
        };
        let is_latest_attempt = previous_usage
            .as_ref()
            .is_none_or(|previous_usage| i64::from(event.attempt) >= previous_usage.attempt);
        let usage_parameters = params![
            event.request_id,
            event.attempt,
            event.local_key_id,
            event.source_id,
            event.candidate_id,
            event.account_id,
            event.requested_model,
            event.resolved_model,
            event.wire_api.as_str(),
            event.transport.as_str(),
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
            event
                .cache_write_ttl
                .as_deref()
                .and_then(zenith_relay_core::usage::normalize_reported_cache_ttls,),
            event.client_context_id,
            upstream_error_json,
            image_usage_json,
        ];
        if record_send {
            transaction
                .prepare_cached(UsageRecordKind::AttemptHistory.upsert_sql())
                .map_err(db_error)?
                .execute(usage_parameters)
                .map_err(db_error)?;
        }
        let mut request_parameters = usage_parameters.to_vec();
        request_parameters[13] = &sql_request_latency_ms;
        request_parameters[14] = &sql_request_ttft_ms;
        let final_request_changed = is_latest_attempt
            && transaction
                .prepare_cached(UsageRecordKind::FinalRequest.upsert_sql())
                .map_err(db_error)?
                .execute(request_parameters.as_slice())
                .map_err(db_error)?
                > 0;
        // Usage belongs to the physical send, not to whichever send completed last.
        // A correction to an older send must update its own model/member bucket.
        if record_send {
            let aggregate = UsageAggregate::from_event(event);
            let previous_aggregate = previous_attempt
                .as_ref()
                .filter(|old| old.aggregated)
                .map(|old| &old.aggregate);
            if previous_aggregate != Some(&aggregate) {
                if let Some(previous_aggregate) = previous_aggregate {
                    apply_aggregate_delta(&transaction, previous_aggregate, -1)?;
                }
                apply_aggregate_delta(&transaction, &aggregate, 1)?;
            }
        }
        // `last_insert_rowid` is unchanged by an upsert that takes the
        // conflict-update path. Only run retention after a new request row;
        // otherwise replacing a request whose old id is divisible by 256
        // would rescan and rewrite the usage database repeatedly.
        let retention_applied = previous_usage.is_none()
            && final_request_changed
            && transaction.last_insert_rowid() % 256 == 0;
        if retention_applied {
            transaction
                .execute_batch(ARCHIVE_USAGE_SQL)
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        if retention_applied {
            self.clear_cached_usage_totals()?;
        } else {
            let mut previous_totals = previous_attempt
                .as_ref()
                .filter(|_| record_send)
                .map(|old| consumption_totals(usage_totals_from_sample(old.totals)))
                .unwrap_or_default();
            let mut incoming_totals = if record_send {
                consumption_totals(usage_totals_from_event(event))
            } else {
                Default::default()
            };
            if final_request_changed {
                if let Some(old) = previous_usage.as_ref() {
                    apply_usage_totals_delta(
                        &mut previous_totals,
                        request_totals(usage_totals_from_sample(old.totals)),
                        true,
                    );
                }
                let logical_totals = usage_totals_from_sample(UsageTotalsSample {
                    success: event.success,
                    latency_ms: request_latency_ms,
                    ttft_ms: request_ttft_ms,
                    generation_ms: event.generation_ms,
                    input_tokens: event.input_tokens,
                    cached_input_tokens: event.cached_input_tokens,
                    cache_write_input_tokens: event.cache_write_input_tokens,
                    reasoning_tokens: event.reasoning_tokens,
                    output_tokens: event.output_tokens,
                    total_tokens: event.total_tokens,
                });
                apply_usage_totals_delta(
                    &mut incoming_totals,
                    request_totals(logical_totals),
                    true,
                );
            }
            self.update_cached_usage_totals(Some(previous_totals), incoming_totals)?;
        }
        if retention_applied {
            // Retention can remove sends belonging to any account.
            self.invalidate_usage_cache();
        } else {
            self.invalidate_usage_cache_for_accounts(
                [
                    event.account_id.as_deref(),
                    previous_usage
                        .as_ref()
                        .and_then(|old| old.aggregate.account_id()),
                    previous_attempt
                        .as_ref()
                        .and_then(|old| old.aggregate.account_id()),
                ]
                .into_iter()
                .flatten(),
            );
        }
        drop(connection);
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        let connection = self.lock_connection()?;
        connection
            .execute_batch("DELETE FROM request_logs; DELETE FROM request_attempts; DELETE FROM usage_candidate_rollups; DELETE FROM weekly_capacity_anchors; DELETE FROM weekly_capacity_history;")
            .map_err(db_error)?;
        self.clear_cached_usage_totals()?;
        *self.quota_samples.lock().map_err(lock_error)? = Default::default();
        self.capacity_seen.lock().map_err(lock_error)?.clear();
        self.capacity_revision.fetch_add(1, Ordering::Release);
        self.invalidate_usage_cache();
        Ok(())
    }

    pub(super) fn invalidate_usage_cache(&self) {
        self.usage_revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cached) = self.usage_report_cache.lock() {
            *cached = None;
        }
        if let Ok(mut cached) = self.api_equivalent_cache.lock() {
            *cached = None;
        }
        if let Ok(mut cached) = self.quota_equivalent_cache.lock() {
            *cached = None;
        }
    }

    fn invalidate_usage_cache_for_accounts<'a>(
        &self,
        account_ids: impl IntoIterator<Item = &'a str>,
    ) {
        self.usage_revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cached) = self.usage_report_cache.lock() {
            *cached = None;
        }
        if let Ok(mut cached) = self.api_equivalent_cache.lock() {
            *cached = None;
        }
        if let Ok(mut cached) = self.quota_equivalent_cache.lock() {
            if let Some(values) = cached.as_mut() {
                for id in account_ids {
                    values.equivalents_by_account.remove(id);
                }
                if values.equivalents_by_account.is_empty() {
                    *cached = None;
                }
            }
        }
    }
}

fn is_local_pre_dispatch_rejection(event: &UsageEvent) -> bool {
    event.attempt == 0
        && event.source_id == "relay"
        && !event.success
        && (400..600).contains(&event.http_status)
        && event.error_origin() == Some(super::ErrorOrigin::Relay)
        && event.candidate_id.is_none()
        && event.account_id.is_none()
        && event.routing.is_none()
        && event.ttft_ms.is_none()
        && event.generation_ms.is_none()
        && event.input_tokens.is_none()
        && event.cached_input_tokens.is_none()
        && event.cache_write_input_tokens.is_none()
        && event.reasoning_tokens.is_none()
        && event.output_tokens.is_none()
        && event.total_tokens.is_none()
        && event.image_usage.is_none()
        && event.upstream_error.is_none()
        && event.quota_snapshot.is_none()
}

#[derive(Clone)]
struct PreviousUsageLog {
    attempt: i64,
    aggregated: bool,
    aggregate: UsageAggregate,
    totals: UsageTotalsSample,
}

fn previous_usage_log(
    transaction: &Transaction<'_>,
    request_id: &str,
    attempt: Option<u16>,
) -> Result<Option<PreviousUsageLog>> {
    let kind = if attempt.is_some() {
        UsageRecordKind::AttemptHistory
    } else {
        UsageRecordKind::FinalRequest
    };
    let parameters = params![request_id, attempt];
    let parameters = if attempt.is_some() {
        parameters
    } else {
        &parameters[..1]
    };
    transaction
        .prepare_cached(kind.previous_sql())
        .map_err(db_error)?
        .query_row(parameters, |row| {
            Ok(PreviousUsageLog {
                attempt: row.get(0)?,
                aggregated: row.get(1)?,
                aggregate: UsageAggregate::from_row_current(row, 2)?,
                totals: UsageTotalsSample {
                    success: row.get(12)?,
                    latency_ms: non_negative_i64(row.get(13)?),
                    ttft_ms: row.get::<_, Option<i64>>(14)?.map(non_negative_i64),
                    generation_ms: row.get::<_, Option<i64>>(15)?.map(non_negative_i64),
                    reasoning_tokens: row.get::<_, Option<i64>>(16)?.map(non_negative_i64),
                    input_tokens: row.get::<_, Option<i64>>(5)?.map(non_negative_i64),
                    cached_input_tokens: row.get::<_, Option<i64>>(6)?.map(non_negative_i64),
                    cache_write_input_tokens: row.get::<_, Option<i64>>(7)?.map(non_negative_i64),
                    output_tokens: row.get::<_, Option<i64>>(9)?.map(non_negative_i64),
                    total_tokens: row.get::<_, Option<i64>>(10)?.map(non_negative_i64),
                },
            })
        })
        .optional()
        .map_err(db_error)
}

fn non_negative_i64(signed_count: i64) -> u64 {
    signed_count.max(0) as u64
}

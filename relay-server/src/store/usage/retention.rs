use super::super::sqlite::{db_error, Store};
use super::query::USAGE_TOTAL_COLUMNS;
use rusqlite::{params, TransactionBehavior};

pub(super) const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

const RAW_USAGE_RETENTION_MS: u64 = 90 * DAY_MS;

const DAILY_ROLLUP_RETENTION_MS: u64 = 400 * DAY_MS;

const MAX_RAW_USAGE_EVENTS: u64 = 100_000;

const USAGE_PRUNE_PREDICATE: &str = "created_at_ms < ?1 OR id NOT IN (
    SELECT id FROM usage_events ORDER BY created_at_ms DESC, id DESC LIMIT ?2
)";

const ROLLUP_USAGE_COLUMNS: &str = "requests, successful_requests, latency_ms,
    ttft_ms, ttft_samples, generation_ms, generation_samples,
    generation_output_tokens, input_tokens, cached_input_tokens,
    cached_input_samples, cache_write_input_tokens, cache_write_input_samples,
    reasoning_tokens, output_tokens, total_tokens, speed_output_tokens,
    speed_duration_ms, input_samples, output_samples, total_samples";

const ROLLUP_UPDATE_COLUMNS: &str = "requests = usage_key_rollups.requests + excluded.requests,
    successful_requests = usage_key_rollups.successful_requests + excluded.successful_requests,
    latency_ms = usage_key_rollups.latency_ms + excluded.latency_ms,
    ttft_ms = usage_key_rollups.ttft_ms + excluded.ttft_ms,
    ttft_samples = usage_key_rollups.ttft_samples + excluded.ttft_samples,
    generation_ms = usage_key_rollups.generation_ms + excluded.generation_ms,
    generation_samples = usage_key_rollups.generation_samples + excluded.generation_samples,
    generation_output_tokens = usage_key_rollups.generation_output_tokens + excluded.generation_output_tokens,
    input_tokens = usage_key_rollups.input_tokens + excluded.input_tokens,
    cached_input_tokens = usage_key_rollups.cached_input_tokens + excluded.cached_input_tokens,
    cached_input_samples = usage_key_rollups.cached_input_samples + excluded.cached_input_samples,
    cache_write_input_tokens = usage_key_rollups.cache_write_input_tokens + excluded.cache_write_input_tokens,
    cache_write_input_samples = usage_key_rollups.cache_write_input_samples + excluded.cache_write_input_samples,
    reasoning_tokens = usage_key_rollups.reasoning_tokens + excluded.reasoning_tokens,
    output_tokens = usage_key_rollups.output_tokens + excluded.output_tokens,
    total_tokens = usage_key_rollups.total_tokens + excluded.total_tokens,
    speed_output_tokens = usage_key_rollups.speed_output_tokens + excluded.speed_output_tokens,
    speed_duration_ms = usage_key_rollups.speed_duration_ms + excluded.speed_duration_ms,
    input_samples = usage_key_rollups.input_samples + excluded.input_samples,
    output_samples = usage_key_rollups.output_samples + excluded.output_samples,
    total_samples = usage_key_rollups.total_samples + excluded.total_samples";

impl Store {
    pub fn prune_usage_history(&self, now_ms: u64) -> Result<usize, String> {
        self.prune_usage_history_with_limits(
            now_ms.saturating_sub(RAW_USAGE_RETENTION_MS),
            MAX_RAW_USAGE_EVENTS,
            now_ms.saturating_sub(DAILY_ROLLUP_RETENTION_MS),
        )
    }

    pub(super) fn prune_usage_history_with_limits(
        &self,
        raw_cutoff_ms: u64,
        max_raw_events: u64,
        daily_rollup_cutoff_ms: u64,
    ) -> Result<usize, String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction
            .execute(
                &format!(
                    "INSERT INTO usage_candidate_rollups(
                        candidate_kind, candidate_id, model,
                        input_tokens, input_samples, cached_input_tokens, cached_input_samples,
                        cache_write_input_tokens, cache_write_input_samples,
                        output_tokens, output_samples, total_tokens, total_samples
                     )
                     SELECT candidate_kind, candidate_hint,
                        COALESCE(resolved_model, requested_model, ''),
                        COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
                        COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
                        COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
                        COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
                        COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
                     FROM usage_events WHERE {USAGE_PRUNE_PREDICATE}
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
                        total_samples=total_samples + excluded.total_samples"
                ),
                params![
                    zenith_relay_core::usage::sql_u64(raw_cutoff_ms),
                    zenith_relay_core::usage::sql_u64(max_raw_events),
                ],
            )
            .map_err(db_error)?;
        for period_sql in ["-1", "(created_at_ms / 86400000) * 86400000"] {
            let sql = format!(
                "INSERT INTO usage_key_rollups(
                    local_key_id, period_start_ms, candidate_kind, candidate_id, model,
                    {ROLLUP_USAGE_COLUMNS}
                 )
                 SELECT local_key_id, {period_sql}, candidate_kind, candidate_hint,
                    COALESCE(resolved_model, requested_model, ''),
                    {USAGE_TOTAL_COLUMNS}, COUNT(input_tokens), COUNT(output_tokens),
                    COUNT(total_tokens)
                 FROM usage_events
                 WHERE {USAGE_PRUNE_PREDICATE}
                 GROUP BY local_key_id, 2, 3, 4, 5
                 ON CONFLICT(local_key_id, period_start_ms, candidate_kind, candidate_id, model)
                 DO UPDATE SET
                    {ROLLUP_UPDATE_COLUMNS}"
            );
            transaction
                .execute(
                    &sql,
                    params![
                        zenith_relay_core::usage::sql_u64(raw_cutoff_ms),
                        zenith_relay_core::usage::sql_u64(max_raw_events),
                    ],
                )
                .map_err(db_error)?;
        }
        transaction
            .execute(
                &format!(
                    "INSERT INTO usage_request_tombstones(request_id, archived_at_ms)
                     SELECT request_id, created_at_ms FROM usage_events
                     WHERE {USAGE_PRUNE_PREDICATE}
                     ON CONFLICT(request_id) DO UPDATE SET
                        archived_at_ms = MAX(usage_request_tombstones.archived_at_ms, excluded.archived_at_ms)"
                ),
                params![
                    zenith_relay_core::usage::sql_u64(raw_cutoff_ms),
                    zenith_relay_core::usage::sql_u64(max_raw_events),
                ],
            )
            .map_err(db_error)?;
        let deleted = transaction
            .execute(
                &format!("DELETE FROM usage_events WHERE {USAGE_PRUNE_PREDICATE}"),
                params![
                    zenith_relay_core::usage::sql_u64(raw_cutoff_ms),
                    zenith_relay_core::usage::sql_u64(max_raw_events),
                ],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "DELETE FROM usage_key_rollups WHERE period_start_ms >= 0 AND period_start_ms < ?1",
                [zenith_relay_core::usage::sql_u64(daily_rollup_cutoff_ms)],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "DELETE FROM usage_request_tombstones WHERE archived_at_ms < ?1",
                [zenith_relay_core::usage::sql_u64(daily_rollup_cutoff_ms)],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(deleted)
    }

    pub fn clear_usage(&self) -> Result<usize, String> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(db_error)?;
        let deleted = transaction
            .execute("DELETE FROM usage_events", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM usage_key_rollups", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM usage_candidate_rollups", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM usage_request_tombstones", [])
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(deleted)
    }
}

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_037: &str = r#"
BEGIN IMMEDIATE;
-- Legacy-send exclusion needs identities, not the full diagnostic row.
CREATE INDEX IF NOT EXISTS request_logs_send_history
    ON request_logs(created_at, request_id, attempt);
-- Normally empty: pricing rollups already contain every retained send.
CREATE INDEX IF NOT EXISTS request_logs_unaggregated
    ON request_logs(usage_aggregate_recorded) WHERE usage_aggregate_recorded = 0;
CREATE INDEX IF NOT EXISTS request_attempts_unaggregated
    ON request_attempts(usage_aggregate_recorded) WHERE usage_aggregate_recorded = 0;
PRAGMA user_version = 37;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_043: &str = r#"
BEGIN IMMEDIATE;
CREATE INDEX IF NOT EXISTS request_attempts_usage_reads ON request_attempts(
    created_at DESC, ABS(id) DESC, request_id, attempt,
    wire_api, transport, error_category, requested_model, resolved_model,
    account_id, source_id, success, latency_ms, ttft_ms, generation_ms,
    input_tokens, cached_input_tokens, cache_write_input_tokens,
    reasoning_tokens, output_tokens, total_tokens,
    applied_service_tier, cache_write_ttl
);
CREATE INDEX IF NOT EXISTS request_logs_usage_reads ON request_logs(
    created_at DESC, ABS(id) DESC, request_id, attempt,
    wire_api, transport, error_category, requested_model, resolved_model,
    account_id, source_id, success, latency_ms, ttft_ms, generation_ms,
    input_tokens, cached_input_tokens, cache_write_input_tokens,
    reasoning_tokens, output_tokens, total_tokens,
    applied_service_tier, cache_write_ttl
);
DROP INDEX IF EXISTS request_attempts_page_order;
DROP INDEX IF EXISTS request_logs_page_order;
DROP INDEX IF EXISTS request_attempts_filter_options;
DROP INDEX IF EXISTS request_logs_filter_options;
PRAGMA user_version = 43;
COMMIT;
"#;

pub(in crate::local_pool::store::telemetry_db) const MIGRATION_044: &str = r#"
BEGIN IMMEDIATE;
CREATE INDEX IF NOT EXISTS request_attempts_account_usage_reads ON request_attempts(
    account_id, created_at, request_id, attempt, source_id,
    requested_model, resolved_model, applied_service_tier,
    input_tokens, cached_input_tokens, cache_write_input_tokens,
    cache_write_ttl, output_tokens, total_tokens
) WHERE account_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS request_logs_account_usage_reads ON request_logs(
    account_id, created_at, request_id, attempt, source_id,
    requested_model, resolved_model, applied_service_tier,
    input_tokens, cached_input_tokens, cache_write_input_tokens,
    cache_write_ttl, output_tokens, total_tokens
) WHERE account_id IS NOT NULL;
DROP INDEX IF EXISTS request_attempts_account_created;
DROP INDEX IF EXISTS request_logs_account_created_idx;
PRAGMA user_version = 44;
COMMIT;
"#;

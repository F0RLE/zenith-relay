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

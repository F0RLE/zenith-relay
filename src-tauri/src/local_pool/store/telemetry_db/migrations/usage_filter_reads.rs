/// Cover retained-history filter choices without reading diagnostic JSON pages.
/// The leading timestamp also bounds the same projection for selected periods.
pub(in crate::local_pool::store::telemetry_db) const MIGRATION_039: &str = r#"
BEGIN IMMEDIATE;
CREATE INDEX IF NOT EXISTS request_attempts_filter_options ON request_attempts(
    created_at, wire_api, transport, error_category, requested_model,
    resolved_model, account_id, source_id
);
CREATE INDEX IF NOT EXISTS request_logs_filter_options ON request_logs(
    created_at, wire_api, transport, error_category, requested_model,
    resolved_model, account_id, source_id, request_id, attempt
);
PRAGMA user_version = 39;
COMMIT;
"#;

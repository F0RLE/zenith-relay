pub(in crate::local_pool::store::telemetry_db) const MIGRATION_042: &str = r#"
BEGIN IMMEDIATE;
CREATE INDEX IF NOT EXISTS request_attempts_page_order
    ON request_attempts(created_at DESC, ABS(id) DESC);
CREATE INDEX IF NOT EXISTS request_logs_page_order
    ON request_logs(created_at DESC, ABS(id) DESC, request_id, attempt);
DROP INDEX IF EXISTS request_attempts_created_at;
DROP INDEX IF EXISTS request_logs_send_history;
PRAGMA user_version = 42;
COMMIT;
"#;

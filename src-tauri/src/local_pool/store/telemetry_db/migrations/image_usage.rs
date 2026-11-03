pub(in crate::local_pool::store::telemetry_db) const MIGRATION_038: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN image_usage_json TEXT;
ALTER TABLE request_attempts ADD COLUMN image_usage_json TEXT;
PRAGMA user_version = 38;
COMMIT;
"#;

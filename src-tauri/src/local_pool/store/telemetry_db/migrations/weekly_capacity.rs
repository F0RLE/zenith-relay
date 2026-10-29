// Local, bounded observations only: no credentials, prompts or provider bodies.
pub(in crate::local_pool::store::telemetry_db) const MIGRATION_041: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE weekly_capacity_anchors (
    account_id TEXT PRIMARY KEY NOT NULL,
    observed_at_ms INTEGER NOT NULL,
    payload_json TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE weekly_capacity_history (
    account_id TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (account_id, observed_at_ms)
) WITHOUT ROWID;
CREATE INDEX weekly_capacity_history_time ON weekly_capacity_history(observed_at_ms);
PRAGMA user_version = 41;
COMMIT;
"#;

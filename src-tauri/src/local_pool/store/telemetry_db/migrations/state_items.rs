// Bounded per-item observations over the saved inventory. Full inventory
// replacement folds these values into app_state and clears its item rows in
// the same transaction. Existing inventories need no eager rewrite.
pub(in crate::local_pool::store::telemetry_db) const MIGRATION_040: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE app_state_items (
    state_key TEXT NOT NULL CHECK(state_key IN ('accounts', 'sources', 'keys')),
    position INTEGER NOT NULL CHECK(position >= 0),
    value_json TEXT NOT NULL,
    PRIMARY KEY(state_key, position)
) WITHOUT ROWID;
PRAGMA user_version = 40;
COMMIT;
"#;

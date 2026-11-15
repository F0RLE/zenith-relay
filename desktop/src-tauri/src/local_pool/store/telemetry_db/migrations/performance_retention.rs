pub(in crate::local_pool::store::telemetry_db) const MIGRATION_045: &str = r#"
BEGIN IMMEDIATE;
DROP TRIGGER performance_samples_retention;
CREATE INDEX IF NOT EXISTS performance_samples_name_id ON performance_samples(name, id DESC);
DELETE FROM performance_samples WHERE created_at < datetime('now', '-30 days');
DELETE FROM performance_samples WHERE id IN (
    SELECT id FROM (
        SELECT id, name, ROW_NUMBER() OVER (PARTITION BY name ORDER BY id DESC) AS position
        FROM performance_samples
    ) WHERE position > CASE WHEN name = 'snapshot_stage' THEN 256 ELSE 128 END
);
CREATE TRIGGER performance_samples_retention
AFTER INSERT ON performance_samples
BEGIN
    DELETE FROM performance_samples WHERE created_at < datetime('now', '-30 days');
    DELETE FROM performance_samples WHERE id IN (
        SELECT id FROM performance_samples WHERE name = NEW.name
        ORDER BY id DESC
        LIMIT -1 OFFSET CASE WHEN NEW.name = 'snapshot_stage' THEN 256 ELSE 128 END
    );
    -- Bound retained rows, not the ID span: pruning one noisy metric leaves gaps.
    DELETE FROM performance_samples WHERE id IN (
        SELECT id FROM performance_samples ORDER BY id DESC LIMIT -1 OFFSET 2048
    );
END;
PRAGMA user_version = 45;
COMMIT;
"#;

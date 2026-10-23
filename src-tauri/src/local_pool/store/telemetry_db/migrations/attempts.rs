pub(in crate::local_pool::store::telemetry_db) const MIGRATION_033: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE request_attempts AS SELECT * FROM request_logs;
UPDATE request_attempts SET id = -rowid;
CREATE UNIQUE INDEX request_attempts_identity ON request_attempts(request_id, attempt);
CREATE INDEX request_attempts_account ON request_attempts(account_id);
CREATE TRIGGER request_attempts_defaults AFTER INSERT ON request_attempts
BEGIN
    UPDATE request_attempts SET id = -NEW.rowid,
        created_at = COALESCE(NEW.created_at, CURRENT_TIMESTAMP)
    WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER request_attempts_retention AFTER DELETE ON request_logs
BEGIN
    DELETE FROM request_attempts WHERE request_id = OLD.request_id;
END;
PRAGMA user_version = 33;
COMMIT;
"#;

CREATE TABLE usage_attempts AS SELECT * FROM usage_events;
UPDATE usage_attempts SET id = -rowid;
CREATE UNIQUE INDEX usage_attempts_identity ON usage_attempts(request_id, attempt);
CREATE INDEX usage_attempts_candidate ON usage_attempts(candidate_kind, candidate_hint);
CREATE TRIGGER usage_attempts_defaults AFTER INSERT ON usage_attempts
BEGIN
    UPDATE usage_attempts SET id = -NEW.rowid WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER usage_attempts_retention AFTER DELETE ON usage_events
BEGIN
    DELETE FROM usage_attempts WHERE request_id = OLD.request_id;
END;

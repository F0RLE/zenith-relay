-- Source incarnations share the monotonic clock, never credential material.
-- A fenced observation changes only its own fields in the latest record. Its
-- sequence exempts that one write from config invalidation, not future edits.
ALTER TABLE sources ADD COLUMN refresh_revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE sources ADD COLUMN observation_sequence INTEGER NOT NULL DEFAULT 0;

CREATE TRIGGER source_refresh_insert AFTER INSERT ON sources BEGIN
    UPDATE refresh_revision_clock SET revision = revision + 1 WHERE id = 1;
    UPDATE sources SET refresh_revision = (SELECT revision FROM refresh_revision_clock WHERE id = 1)
    WHERE id = NEW.id;
END;

CREATE TRIGGER source_refresh_config AFTER UPDATE OF data_json, secret_ref ON sources
WHEN OLD.observation_sequence = NEW.observation_sequence AND (
    OLD.secret_ref IS NOT NEW.secret_ref
    OR json_extract(OLD.data_json, '$.enabled') IS NOT json_extract(NEW.data_json, '$.enabled')
    OR json_extract(OLD.data_json, '$.baseUrl') IS NOT json_extract(NEW.data_json, '$.baseUrl')
    OR json_extract(OLD.data_json, '$.wireApi') IS NOT json_extract(NEW.data_json, '$.wireApi')
    OR json_extract(OLD.data_json, '$.protocolBindings') IS NOT json_extract(NEW.data_json, '$.protocolBindings')
    OR json_extract(OLD.data_json, '$.protocolConfig') IS NOT json_extract(NEW.data_json, '$.protocolConfig')
    OR json_extract(OLD.data_json, '$.models') IS NOT json_extract(NEW.data_json, '$.models')
)
BEGIN
    UPDATE refresh_revision_clock SET revision = revision + 1 WHERE id = 1;
    UPDATE sources SET refresh_revision = (SELECT revision FROM refresh_revision_clock WHERE id = 1)
    WHERE id = NEW.id;
END;

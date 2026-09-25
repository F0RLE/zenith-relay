-- Observation fences live outside serialized user configuration. Incarnations
-- remain unique after delete/re-add, while quota/usage updates do not invalidate
-- independent model reads. No credential material is stored here.
CREATE TABLE refresh_revision_clock (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    revision INTEGER NOT NULL
);
INSERT INTO refresh_revision_clock VALUES (1, 1);
ALTER TABLE accounts ADD COLUMN refresh_revision INTEGER NOT NULL DEFAULT 1;
INSERT INTO metadata(key, value) VALUES ('refresh_config_revision', '1');

CREATE TRIGGER account_refresh_insert AFTER INSERT ON accounts BEGIN
    UPDATE refresh_revision_clock SET revision = revision + 1 WHERE id = 1;
    UPDATE accounts SET refresh_revision = (SELECT revision FROM refresh_revision_clock WHERE id = 1)
    WHERE id = NEW.id;
END;

CREATE TRIGGER account_refresh_config AFTER UPDATE OF data_json, secret_ref ON accounts
WHEN OLD.secret_ref IS NOT NEW.secret_ref
    OR json_extract(OLD.data_json, '$.enabled') IS NOT json_extract(NEW.data_json, '$.enabled')
    OR json_extract(OLD.data_json, '$.sourceId') IS NOT json_extract(NEW.data_json, '$.sourceId')
    OR json_extract(OLD.data_json, '$.proxyId') IS NOT json_extract(NEW.data_json, '$.proxyId')
    OR COALESCE(json_extract(OLD.data_json, '$.bypassCommonProxy'), 0) IS NOT COALESCE(json_extract(NEW.data_json, '$.bypassCommonProxy'), 0)
    OR COALESCE(json_extract(OLD.data_json, '$.createdAtMs'), 0) IS NOT COALESCE(json_extract(NEW.data_json, '$.createdAtMs'), 0)
BEGIN
    UPDATE refresh_revision_clock SET revision = revision + 1 WHERE id = 1;
    UPDATE accounts SET refresh_revision = (SELECT revision FROM refresh_revision_clock WHERE id = 1)
    WHERE id = NEW.id;
END;

CREATE TRIGGER account_refresh_global_insert AFTER INSERT ON metadata
WHEN NEW.key IN ('common_proxy_id', 'common_proxy_configured', 'account_proxy_required', 'quota_request_timeout_seconds')
BEGIN
    UPDATE refresh_revision_clock SET revision = revision + 1 WHERE id = 1;
    UPDATE metadata SET value = CAST((SELECT revision FROM refresh_revision_clock WHERE id = 1) AS TEXT)
    WHERE key = 'refresh_config_revision';
END;

CREATE TRIGGER account_refresh_global_update AFTER UPDATE OF value ON metadata
WHEN NEW.key IN ('common_proxy_id', 'common_proxy_configured', 'account_proxy_required', 'quota_request_timeout_seconds')
    AND OLD.value IS NOT NEW.value
BEGIN
    UPDATE refresh_revision_clock SET revision = revision + 1 WHERE id = 1;
    UPDATE metadata SET value = CAST((SELECT revision FROM refresh_revision_clock WHERE id = 1) AS TEXT)
    WHERE key = 'refresh_config_revision';
END;

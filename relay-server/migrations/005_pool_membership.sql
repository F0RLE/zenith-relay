UPDATE sources
SET data_json = json_set(data_json, '$.inPool', json('false'))
WHERE json_type(data_json, '$.inPool') IS NULL;

UPDATE accounts
SET data_json = json_set(data_json, '$.inPool', json('false'))
WHERE json_type(data_json, '$.inPool') IS NULL;

INSERT OR IGNORE INTO metadata(key, value)
VALUES ('quota_refresh_interval_seconds', '300');

INSERT OR IGNORE INTO metadata(key, value)
VALUES ('quota_request_timeout_seconds', '20');

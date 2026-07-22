CREATE TABLE IF NOT EXISTS proxies (
    id TEXT PRIMARY KEY,
    data_json TEXT NOT NULL,
    secret_ref TEXT NOT NULL UNIQUE
);

INSERT OR IGNORE INTO metadata(key, value) VALUES ('common_proxy_id', '');

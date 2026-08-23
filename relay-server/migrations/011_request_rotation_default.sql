INSERT INTO metadata(key, value)
VALUES ('session_affinity', 'false')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

ALTER TABLE usage_events ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;

DROP INDEX IF EXISTS idx_usage_request_id;
DELETE FROM usage_events
WHERE id NOT IN (
    SELECT MAX(id) FROM usage_events GROUP BY request_id
);
CREATE UNIQUE INDEX idx_usage_request_id ON usage_events(request_id);

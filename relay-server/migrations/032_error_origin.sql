ALTER TABLE usage_events ADD COLUMN error_origin TEXT;
CREATE INDEX IF NOT EXISTS idx_usage_error_origin
    ON usage_events(error_origin, created_at_ms DESC);

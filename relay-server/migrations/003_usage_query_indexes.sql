CREATE INDEX IF NOT EXISTS idx_usage_request_id ON usage_events(request_id);
CREATE INDEX IF NOT EXISTS idx_usage_requested_model ON usage_events(requested_model, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_usage_key ON usage_events(local_key_id, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_usage_candidate ON usage_events(candidate_hint, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_usage_wire_api ON usage_events(wire_api, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_usage_error ON usage_events(error_category, created_at_ms DESC);

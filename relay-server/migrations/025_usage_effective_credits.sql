ALTER TABLE usage_events ADD COLUMN service_tier TEXT NOT NULL DEFAULT 'standard';
ALTER TABLE usage_events ADD COLUMN effective_credits_milli INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_key_rollups ADD COLUMN effective_credits_milli INTEGER NOT NULL DEFAULT 0;

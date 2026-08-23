CREATE TABLE usage_key_rollups (
    local_key_id TEXT NOT NULL,
    period_start_ms INTEGER NOT NULL,
    model TEXT NOT NULL,
    requests INTEGER NOT NULL DEFAULT 0,
    successful_requests INTEGER NOT NULL DEFAULT 0,
    latency_ms INTEGER NOT NULL DEFAULT 0,
    ttft_ms INTEGER NOT NULL DEFAULT 0,
    ttft_samples INTEGER NOT NULL DEFAULT 0,
    generation_ms INTEGER NOT NULL DEFAULT 0,
    generation_samples INTEGER NOT NULL DEFAULT 0,
    generation_output_tokens INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    input_samples INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_samples INTEGER NOT NULL DEFAULT 0,
    cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_input_samples INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    output_samples INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_samples INTEGER NOT NULL DEFAULT 0,
    speed_output_tokens INTEGER NOT NULL DEFAULT 0,
    speed_duration_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(local_key_id, period_start_ms, model)
) WITHOUT ROWID;

CREATE INDEX idx_usage_key_rollups_period
    ON usage_key_rollups(period_start_ms, local_key_id);

CREATE TABLE usage_request_tombstones (
    request_id TEXT PRIMARY KEY,
    archived_at_ms INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX idx_usage_request_tombstones_archived
    ON usage_request_tombstones(archived_at_ms);

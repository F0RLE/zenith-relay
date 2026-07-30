ALTER TABLE usage_key_rollups RENAME TO usage_key_rollups_legacy;

CREATE TABLE usage_key_rollups (
    local_key_id TEXT NOT NULL,
    period_start_ms INTEGER NOT NULL,
    candidate_kind TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
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
    PRIMARY KEY(local_key_id, period_start_ms, candidate_kind, candidate_id, model)
) WITHOUT ROWID;

INSERT INTO usage_key_rollups(
    local_key_id, period_start_ms, candidate_kind, candidate_id, model,
    requests, successful_requests, latency_ms, ttft_ms, ttft_samples,
    generation_ms, generation_samples, generation_output_tokens,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples, reasoning_tokens,
    output_tokens, output_samples, total_tokens, total_samples,
    speed_output_tokens, speed_duration_ms
)
SELECT local_key_id, period_start_ms, '', '', model,
    requests, successful_requests, latency_ms, ttft_ms, ttft_samples,
    generation_ms, generation_samples, generation_output_tokens,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples, reasoning_tokens,
    output_tokens, output_samples, total_tokens, total_samples,
    speed_output_tokens, speed_duration_ms
FROM usage_key_rollups_legacy;

DROP TABLE usage_key_rollups_legacy;

CREATE INDEX idx_usage_key_rollups_period
    ON usage_key_rollups(period_start_ms, local_key_id);

mod early;
mod late;

pub(super) use early::*;
pub(super) use late::*;

pub(super) const LOCAL_DATABASE_SCHEMA_VERSION: u32 = 29;
pub(super) const MAX_RESPONSE_AFFINITY_ROWS: usize = 16_384;
pub(super) const MAX_STATE_JSON_BYTES: usize = 16 * 1024 * 1024;
pub(super) const ARCHIVE_USAGE_SQL: &str = r#"
INSERT INTO usage_candidate_rollups(
    candidate_kind, candidate_id, model,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples,
    cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
    output_tokens, output_samples, total_tokens, total_samples
)
SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
    COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
    COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
    COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h')
        THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
    COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
FROM request_logs
WHERE created_at < datetime('now', '-30 days')
    AND usage_aggregate_recorded = 0
GROUP BY 1, 2, 3
ON CONFLICT(candidate_kind, candidate_id, model) DO UPDATE SET
    input_tokens = input_tokens + excluded.input_tokens,
    input_samples = input_samples + excluded.input_samples,
    cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
    cached_input_samples = cached_input_samples + excluded.cached_input_samples,
    cache_write_input_tokens = cache_write_input_tokens + excluded.cache_write_input_tokens,
    cache_write_input_samples = cache_write_input_samples + excluded.cache_write_input_samples,
    cache_write_5m_tokens = cache_write_5m_tokens + excluded.cache_write_5m_tokens,
    cache_write_1h_tokens = cache_write_1h_tokens + excluded.cache_write_1h_tokens,
    unknown_cache_write_tokens = unknown_cache_write_tokens + excluded.unknown_cache_write_tokens,
    output_tokens = output_tokens + excluded.output_tokens,
    output_samples = output_samples + excluded.output_samples,
    total_tokens = total_tokens + excluded.total_tokens,
    total_samples = total_samples + excluded.total_samples;
DELETE FROM request_logs WHERE created_at < datetime('now', '-30 days');
"#;

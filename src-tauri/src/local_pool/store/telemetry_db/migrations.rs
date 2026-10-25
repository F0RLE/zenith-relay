mod attempts;
mod context_bands;
mod early;
mod image_usage;
mod late;
mod send_usage;
mod service_tiers;
mod state_items;
mod unknown_tiers;
mod usage_filter_reads;
mod usage_reads;
mod weekly_capacity;

pub(super) use attempts::MIGRATION_033;
pub(super) use context_bands::migrate_032;
pub(super) use early::*;
pub(super) use image_usage::MIGRATION_038;
pub(super) use late::*;
pub(super) use send_usage::migrate_036;
pub(super) use service_tiers::migrate_034;
pub(super) use state_items::MIGRATION_040;
pub(super) use unknown_tiers::migrate_035;
pub(super) use usage_filter_reads::MIGRATION_039;
pub(super) use usage_reads::MIGRATION_037;
pub(super) use weekly_capacity::MIGRATION_041;

pub(super) const LOCAL_DATABASE_SCHEMA_VERSION: u32 = 41;
pub(super) const MAX_RESPONSE_AFFINITY_ROWS: usize = 16_384;
pub(super) const MAX_STATE_JSON_BYTES: usize = 16 * 1024 * 1024;
pub(super) const ARCHIVE_USAGE_SQL: &str = concat!(
    r#"
-- Capture one stable oldest-first batch. Archiving and deleting must use
-- exactly these rows, even if the retention boundary advances meanwhile.
-- The caller owns the transaction; this temporary table never leaves SQLite.
CREATE TEMP TABLE IF NOT EXISTS usage_retention_batch(request_id TEXT PRIMARY KEY);
DELETE FROM usage_retention_batch;
INSERT INTO usage_retention_batch(request_id)
SELECT request_id FROM request_logs
WHERE created_at < datetime('now', '-30 days')
ORDER BY created_at, id LIMIT 256;
INSERT INTO usage_candidate_rollups(
    candidate_kind, candidate_id, model,
    price_class, context_band,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples,
    cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
    output_tokens, output_samples, total_tokens, total_samples
)
SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
    "#,
    zenith_relay_core::usage_price_class_sql!(),
    ", ",
    zenith_relay_core::usage_context_band_sql!(),
    r#",
    COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
    COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
    COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h')
        THEN cache_write_input_tokens ELSE 0 END), 0),
    COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
    COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
FROM usage_sends
WHERE request_id IN (SELECT request_id FROM usage_retention_batch)
    AND usage_aggregate_recorded = 0
GROUP BY 1, 2, 3, 4, 5
ON CONFLICT(candidate_kind, candidate_id, model, price_class, context_band) DO UPDATE SET
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
DELETE FROM request_logs WHERE request_id IN (SELECT request_id FROM usage_retention_batch);
DELETE FROM usage_retention_batch;
"#
);

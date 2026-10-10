-- Split retained usage by the observed service tier and the prompt-size band.
-- Events still stored are rebuilt exactly. Tokens whose raw event was already
-- deleted stay on the standard/base schedule.
CREATE TABLE usage_candidate_rollups_v2 (
    candidate_kind TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    model TEXT NOT NULL,
    price_class TEXT NOT NULL DEFAULT 'standard',
    context_band TEXT NOT NULL DEFAULT 'base',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    input_samples INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_input_samples INTEGER NOT NULL DEFAULT 0,
    cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_input_samples INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    output_samples INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_samples INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(candidate_kind, candidate_id, model, price_class, context_band)
) WITHOUT ROWID;

INSERT INTO usage_candidate_rollups_v2(
    candidate_kind, candidate_id, model, price_class, context_band,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples,
    output_tokens, output_samples, total_tokens, total_samples
)
SELECT candidate_kind, candidate_hint,
    COALESCE(resolved_model, requested_model, ''),
    CASE lower(COALESCE(applied_service_tier, ''))
        WHEN 'flex' THEN 'flex'
        WHEN 'priority' THEN 'priority'
        WHEN 'fast' THEN 'priority'
        ELSE 'standard' END,
    CASE
        WHEN COALESCE(input_tokens, 0) > 272000 THEN 'above_272k'
        WHEN COALESCE(input_tokens, 0) > 200000 THEN 'above_200k'
        ELSE 'base' END,
    COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
    COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
    COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
    COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
    COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
FROM usage_events
GROUP BY 1, 2, 3, 4, 5;

INSERT INTO usage_candidate_rollups_v2(
    candidate_kind, candidate_id, model, price_class, context_band,
    input_tokens, input_samples, cached_input_tokens, cached_input_samples,
    cache_write_input_tokens, cache_write_input_samples,
    output_tokens, output_samples, total_tokens, total_samples
)
SELECT old.candidate_kind, old.candidate_id, old.model, 'standard', 'base',
    MAX(old.input_tokens - COALESCE(fresh.input_tokens, 0), 0),
    MAX(old.input_samples - COALESCE(fresh.input_samples, 0), 0),
    MAX(old.cached_input_tokens - COALESCE(fresh.cached_input_tokens, 0), 0),
    MAX(old.cached_input_samples - COALESCE(fresh.cached_input_samples, 0), 0),
    MAX(old.cache_write_input_tokens - COALESCE(fresh.cache_write_input_tokens, 0), 0),
    MAX(old.cache_write_input_samples - COALESCE(fresh.cache_write_input_samples, 0), 0),
    MAX(old.output_tokens - COALESCE(fresh.output_tokens, 0), 0),
    MAX(old.output_samples - COALESCE(fresh.output_samples, 0), 0),
    MAX(old.total_tokens - COALESCE(fresh.total_tokens, 0), 0),
    MAX(old.total_samples - COALESCE(fresh.total_samples, 0), 0)
FROM usage_candidate_rollups AS old
LEFT JOIN (
    SELECT candidate_kind, candidate_id, model,
        SUM(input_tokens) AS input_tokens,
        SUM(input_samples) AS input_samples,
        SUM(cached_input_tokens) AS cached_input_tokens,
        SUM(cached_input_samples) AS cached_input_samples,
        SUM(cache_write_input_tokens) AS cache_write_input_tokens,
        SUM(cache_write_input_samples) AS cache_write_input_samples,
        SUM(output_tokens) AS output_tokens,
        SUM(output_samples) AS output_samples,
        SUM(total_tokens) AS total_tokens,
        SUM(total_samples) AS total_samples
    FROM usage_candidate_rollups_v2
    GROUP BY candidate_kind, candidate_id, model
) AS fresh
    ON fresh.candidate_kind = old.candidate_kind
    AND fresh.candidate_id = old.candidate_id
    AND fresh.model = old.model
WHERE old.input_tokens > COALESCE(fresh.input_tokens, 0)
    OR old.input_samples > COALESCE(fresh.input_samples, 0)
    OR old.cached_input_samples > COALESCE(fresh.cached_input_samples, 0)
    OR old.cache_write_input_samples > COALESCE(fresh.cache_write_input_samples, 0)
    OR old.output_tokens > COALESCE(fresh.output_tokens, 0)
    OR old.output_samples > COALESCE(fresh.output_samples, 0)
    OR old.total_tokens > COALESCE(fresh.total_tokens, 0)
    OR old.total_samples > COALESCE(fresh.total_samples, 0)
    OR old.cached_input_tokens > COALESCE(fresh.cached_input_tokens, 0)
    OR old.cache_write_input_tokens > COALESCE(fresh.cache_write_input_tokens, 0)
ON CONFLICT(candidate_kind, candidate_id, model, price_class, context_band) DO UPDATE SET
    input_tokens = input_tokens + excluded.input_tokens,
    input_samples = input_samples + excluded.input_samples,
    cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
    cached_input_samples = cached_input_samples + excluded.cached_input_samples,
    cache_write_input_tokens = cache_write_input_tokens + excluded.cache_write_input_tokens,
    cache_write_input_samples = cache_write_input_samples + excluded.cache_write_input_samples,
    output_tokens = output_tokens + excluded.output_tokens,
    output_samples = output_samples + excluded.output_samples,
    total_tokens = total_tokens + excluded.total_tokens,
    total_samples = total_samples + excluded.total_samples;

DROP TABLE usage_candidate_rollups;
ALTER TABLE usage_candidate_rollups_v2 RENAME TO usage_candidate_rollups;

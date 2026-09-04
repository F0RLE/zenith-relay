use super::UsagePriceResolver;
use super::{db_error, UsageLog};
use crate::local_pool::error::Result;
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use std::collections::HashMap;
use zenith_relay_core::{
    pricing::PriceSource,
    protocol::{UsageBucket, UsageGroup, UsageQuery, UsageTotals},
    sql_like_contains_pattern, ApiEquivalentSummary, ApiEquivalentUsage, DefaultServiceTier,
    UsageEvent, WireApi,
};

/// The scalar fields that contribute to a totals row for one request.
///
/// Keeping this projection next to the SQL aggregate definition makes the
/// incremental totals cache use the same accounting rules as a full scan.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct UsageTotalsSample {
    pub(super) success: bool,
    pub(super) latency_ms: u64,
    pub(super) ttft_ms: Option<u64>,
    pub(super) generation_ms: Option<u64>,
    pub(super) input_tokens: Option<u64>,
    pub(super) cached_input_tokens: Option<u64>,
    pub(super) cache_write_input_tokens: Option<u64>,
    pub(super) reasoning_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) total_tokens: Option<u64>,
}

pub(super) fn usage_totals_from_event(event: &UsageEvent) -> UsageTotals {
    usage_totals_from_sample(UsageTotalsSample {
        success: event.success,
        latency_ms: event.latency_ms,
        ttft_ms: event.ttft_ms,
        generation_ms: event.generation_ms,
        input_tokens: event.input_tokens,
        cached_input_tokens: event.cached_input_tokens,
        cache_write_input_tokens: event.cache_write_input_tokens,
        reasoning_tokens: event.reasoning_tokens,
        output_tokens: event.output_tokens,
        total_tokens: event.total_tokens,
    })
}

pub(super) fn usage_totals_from_sample(sample: UsageTotalsSample) -> UsageTotals {
    let mut totals = UsageTotals {
        requests: 1,
        successful_requests: u64::from(sample.success),
        latency_ms: sample.latency_ms,
        input_tokens: sample.input_tokens.unwrap_or_default(),
        cached_input_tokens: sample.cached_input_tokens.unwrap_or_default(),
        reasoning_tokens: sample.reasoning_tokens.unwrap_or_default(),
        output_tokens: sample.output_tokens.unwrap_or_default(),
        total_tokens: sample.total_tokens.unwrap_or_default(),
        ..UsageTotals::default()
    };
    if let Some(ttft_ms) = sample.ttft_ms {
        totals.ttft_ms = ttft_ms;
        totals.ttft_samples = 1;
    }
    if let Some(cache_write_input_tokens) = sample.cache_write_input_tokens {
        totals.cache_write_input_tokens = cache_write_input_tokens;
        totals.cache_write_input_samples = 1;
    }
    if let Some(generation_ms) = sample.generation_ms {
        let generation_output_tokens = sample
            .output_tokens
            .unwrap_or_default()
            .saturating_sub(sample.reasoning_tokens.unwrap_or_default())
            .saturating_sub(1);
        if sample.success
            && generation_ms > 0
            && generation_output_tokens > 0
            // One token per millisecond is the upper bound for a reliable
            // generation-throughput sample. Faster values usually indicate
            // buffered output released at the end of the request.
            && generation_output_tokens <= generation_ms
        {
            totals.generation_ms = generation_ms;
            totals.generation_samples = 1;
            totals.generation_output_tokens = generation_output_tokens;
        }
    }
    if sample.success
        && sample.output_tokens.is_some_and(|tokens| tokens > 0)
        && sample.latency_ms > 0
        && sample.output_tokens.unwrap_or_default() <= sample.latency_ms
    {
        totals.speed_output_tokens = sample.output_tokens.unwrap_or_default();
        totals.speed_duration_ms = sample.latency_ms;
    }
    totals
}

pub(super) fn apply_usage_totals_delta(target: &mut UsageTotals, delta: UsageTotals, add: bool) {
    macro_rules! adjust {
        ($field:ident) => {
            target.$field = if add {
                target.$field.saturating_add(delta.$field)
            } else {
                target.$field.saturating_sub(delta.$field)
            };
        };
    }
    adjust!(requests);
    adjust!(successful_requests);
    adjust!(latency_ms);
    adjust!(ttft_ms);
    adjust!(ttft_samples);
    adjust!(generation_ms);
    adjust!(generation_samples);
    adjust!(generation_output_tokens);
    adjust!(input_tokens);
    adjust!(cached_input_tokens);
    adjust!(cached_input_samples);
    adjust!(cache_write_input_tokens);
    adjust!(cache_write_input_samples);
    adjust!(reasoning_tokens);
    adjust!(output_tokens);
    adjust!(total_tokens);
    adjust!(speed_output_tokens);
    adjust!(speed_duration_ms);
}

pub(super) fn is_unfiltered_all_time(query: &UsageQuery) -> bool {
    query.range.is_none()
        && query.from_ms.is_none()
        && query.to_ms.is_none()
        && query.bucket_ms.is_none()
        && query.model_query.is_none()
        && query.source_or_account_query.is_none()
        && query.wire_api.is_none()
        && query.success.is_none()
        && query.error_category.is_none()
        && query.request_id_query.is_none()
}

const USAGE_TOTAL_COLUMNS: &str = "COUNT(*), \
    COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
    COALESCE(SUM(latency_ms), 0), COALESCE(SUM(ttft_ms), 0), COUNT(ttft_ms), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) <= generation_ms \
        THEN generation_ms ELSE 0 END), 0), \
    COUNT(CASE WHEN success != 0 AND generation_ms > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) <= generation_ms \
        THEN generation_ms END), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) <= generation_ms \
        THEN MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) ELSE 0 END), 0), \
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0), \
    COUNT(cached_input_tokens), COALESCE(SUM(cache_write_input_tokens), 0), \
    COUNT(cache_write_input_tokens), COALESCE(SUM(reasoning_tokens), 0), \
    COALESCE(SUM(output_tokens), 0), \
    COALESCE(SUM(total_tokens), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 \
        AND COALESCE(output_tokens, 0) <= latency_ms \
        THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 \
        AND COALESCE(output_tokens, 0) <= latency_ms \
        THEN latency_ms ELSE 0 END), 0)";

/// Aggregate columns used when only API-equivalent pricing is needed. The
/// sample counts are important: a cache value is only treated as complete
/// when every row in the aggregate reported that measurement.
const USAGE_PRICING_AGGREGATE_COLUMNS: &str = "SUM(input_tokens), SUM(cached_input_tokens), \
    SUM(cache_write_input_tokens), \
    SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), \
    SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), \
    SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h') \
        THEN cache_write_input_tokens ELSE 0 END), \
    SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens), \
    COUNT(cached_input_tokens), COUNT(cache_write_input_tokens), \
    COUNT(output_tokens), COUNT(total_tokens)";

pub(super) fn usage_filter(query: &UsageQuery) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(value) = query.from_ms {
        clauses.push("created_at >= datetime(? / 1000, 'unixepoch')");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.to_ms {
        clauses.push("created_at <= datetime(? / 1000, 'unixepoch')");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.model_query.as_deref() {
        clauses.push("(requested_model LIKE ? ESCAPE '\\' OR resolved_model LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(sql_like_contains_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.source_or_account_query.as_deref() {
        clauses.push("(source_id LIKE ? ESCAPE '\\' OR account_id LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(sql_like_contains_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.wire_api {
        match value {
            WireApi::ChatCompletions => {
                clauses.push("wire_api IN (?, ?)");
                values.push(SqlValue::Text("chat_completions".to_string()));
                values.push(SqlValue::Text("chatcompletions".to_string()));
            }
            _ => {
                clauses.push("wire_api = ?");
                values.push(SqlValue::Text(value.as_str().to_string()));
            }
        }
    }
    if let Some(value) = query.success {
        clauses.push("success = ?");
        values.push(SqlValue::Integer(i64::from(value)));
    }
    if let Some(value) = query.error_category.as_deref() {
        clauses.push("error_category = ?");
        values.push(SqlValue::Text(value.to_string()));
    }
    if let Some(value) = query.request_id_query.as_deref() {
        clauses.push("request_id LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(sql_like_contains_pattern(value)));
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, values)
}

pub(super) fn usage_totals(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
) -> Result<UsageTotals> {
    let sql = format!("SELECT {USAGE_TOTAL_COLUMNS} FROM request_logs{where_sql}");
    connection
        .query_row(&sql, params_from_iter(values.iter()), |row| {
            usage_totals_from_row(row, 0)
        })
        .map_err(db_error)
}

/// Load the minimal per-model aggregates needed for one account quota window.
///
/// Quota projections are an internal exact-id lookup, unlike the user-facing
/// search filter. Keeping the predicate sargable lets SQLite use the
/// `(account_id, created_at)` index and avoids materializing request events or
/// unrelated source rows.
pub(super) fn account_pricing_aggregates(
    connection: &Connection,
    account_id: &str,
    from_ms: u64,
    to_ms: u64,
) -> Result<Vec<(String, ApiEquivalentUsage)>> {
    let sql = format!(
        "SELECT COALESCE(resolved_model, requested_model, ''), \
            {USAGE_PRICING_AGGREGATE_COLUMNS} \
         FROM request_logs \
         WHERE account_id = ?1 \
           AND created_at >= datetime(?2 / 1000, 'unixepoch') \
           AND created_at <= datetime(?3 / 1000, 'unixepoch') \
         GROUP BY 1"
    );
    let values = [
        SqlValue::Text(account_id.to_string()),
        SqlValue::Integer(from_ms.min(i64::MAX as u64) as i64),
        SqlValue::Integer(to_ms.min(i64::MAX as u64) as i64),
    ];
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok((row.get(0)?, usage_pricing_usage_from_row(row, 1)?))
        })
        .map_err(db_error)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)
}

pub(super) fn usage_groups(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    key_sql: &str,
) -> Result<Vec<UsageGroup>> {
    let sql = format!(
        "SELECT {key_sql}, {USAGE_TOTAL_COLUMNS} FROM request_logs{where_sql} \
         GROUP BY 1 ORDER BY COUNT(*) DESC, 1"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok(UsageGroup {
                key: row.get(0)?,
                label: None,
                totals: usage_totals_from_row(row, 1)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)
}

pub(super) fn usage_model_equivalents(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    resolver: &dyn UsagePriceResolver,
    use_rollup: bool,
) -> Result<(HashMap<String, ApiEquivalentSummary>, Vec<PriceSource>)> {
    let sql = if use_rollup && where_sql.is_empty() {
        // Current rows are maintained transactionally in the rollup. Keep a
        // small compatibility branch for rows written by an older Relay
        // version (or a recovery tool) before the aggregate flag existed.
        "SELECT candidate_kind, candidate_id, model,
                input_tokens, cached_input_tokens, cache_write_input_tokens,
                cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
                output_tokens, total_tokens, input_samples,
                cached_input_samples, cache_write_input_samples,
                output_samples, total_samples
             FROM usage_candidate_rollups
             UNION ALL
             SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
                SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END),
                SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END),
                SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h')
                    THEN cache_write_input_tokens ELSE 0 END),
                SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens),
                COUNT(cached_input_tokens), COUNT(cache_write_input_tokens),
                COUNT(output_tokens), COUNT(total_tokens)
             FROM request_logs
             WHERE usage_aggregate_recorded = 0
             GROUP BY 1, 2, 3"
            .to_string()
    } else {
        format!(
            "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                {USAGE_PRICING_AGGREGATE_COLUMNS}
             FROM request_logs{where_sql} GROUP BY 1, 2, 3"
        )
    };
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            let kind = row.get::<_, String>(0)?;
            let candidate_id = row.get::<_, String>(1)?;
            let model = row.get::<_, String>(2)?;
            let model_ref = (!model.is_empty()).then_some(model.as_str());
            let estimate = resolver.estimate(
                &kind,
                &candidate_id,
                model_ref,
                usage_pricing_usage_from_row(row, 3)?,
            );
            let source = resolver.source(&kind, &candidate_id, model_ref);
            Ok((model.clone(), estimate, source))
        })
        .map_err(db_error)?;
    let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
    let mut sources = Vec::new();
    for row in rows {
        let (model, estimate, source) = row.map_err(db_error)?;
        equivalents.entry(model).or_default().merge(estimate);
        sources.push(source);
    }
    Ok((equivalents, sources))
}

pub(super) fn usage_buckets(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    query: &UsageQuery,
    resolver: &dyn UsagePriceResolver,
) -> Result<Vec<UsageBucket>> {
    let Some(bucket_ms) = query.bucket_ms else {
        return Ok(Vec::new());
    };
    let start_ms = query.from_ms.unwrap_or_default();
    let start = SqlValue::Integer(start_ms.min(i64::MAX as u64) as i64);
    let bucket = SqlValue::Integer(bucket_ms.min(i64::MAX as u64) as i64);
    let bucket_sql = "? + ((CAST(strftime('%s', created_at) AS INTEGER) * 1000 - ?) / ?) * ?";
    let sql = format!(
        "SELECT {bucket_sql}, {USAGE_TOTAL_COLUMNS} \
         FROM request_logs{where_sql} GROUP BY 1 ORDER BY 1"
    );
    let mut parameters = vec![start.clone(), start, bucket.clone(), bucket];
    parameters.extend_from_slice(values);
    let mut buckets = {
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok(UsageBucket {
                    start_ms: rust_u64(row.get(0)?),
                    totals: usage_totals_from_row(row, 1)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?
    };
    let price_sql = format!(
        "SELECT {bucket_sql}, CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END, \
            COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model), \
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens), \
            SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), \
            SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), \
            SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h') THEN cache_write_input_tokens ELSE 0 END), \
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens), \
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens), \
            COUNT(output_tokens), COUNT(total_tokens) \
         FROM request_logs{where_sql} GROUP BY 1, 2, 3, 4"
    );
    let mut statement = connection.prepare(&price_sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            let kind = row.get::<_, String>(1)?;
            let candidate_id = row.get::<_, String>(2)?;
            let model = row.get::<_, Option<String>>(3)?;
            let start_ms = rust_u64(row.get(0)?);
            Ok((
                start_ms,
                resolver.estimate(
                    &kind,
                    &candidate_id,
                    model.as_deref(),
                    usage_pricing_usage_from_row(row, 4)?,
                ),
            ))
        })
        .map_err(db_error)?;
    let mut equivalents = HashMap::<u64, ApiEquivalentSummary>::new();
    for row in rows {
        let (start_ms, estimate) = row.map_err(db_error)?;
        equivalents.entry(start_ms).or_default().merge(estimate);
    }
    for bucket in &mut buckets {
        bucket.totals.api_equivalent = equivalents.remove(&bucket.start_ms).unwrap_or_default();
    }
    Ok(buckets)
}

fn usage_pricing_usage_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ApiEquivalentUsage> {
    let input_tokens = row.get::<_, Option<i64>>(offset)?.map(rust_u64);
    let cached_input_tokens = row.get::<_, Option<i64>>(offset + 1)?.map(rust_u64);
    let cache_write_5m_tokens = row.get::<_, Option<i64>>(offset + 3)?.map(rust_u64);
    let cache_write_1h_tokens = row.get::<_, Option<i64>>(offset + 4)?.map(rust_u64);
    let unknown_cache_write_tokens = row.get::<_, Option<i64>>(offset + 5)?.map(rust_u64);
    let output_tokens = row.get::<_, Option<i64>>(offset + 6)?.map(rust_u64);
    let total_tokens = row.get::<_, Option<i64>>(offset + 7)?.map(rust_u64);
    let input_samples = rust_u64(row.get(offset + 8)?);
    let cached_samples = rust_u64(row.get(offset + 9)?);
    let cache_write_samples = rust_u64(row.get(offset + 10)?);
    let output_samples = rust_u64(row.get(offset + 11)?);
    let total_samples = rust_u64(row.get(offset + 12)?);
    let cache_writes = cache_write_samples > 0;
    Ok(ApiEquivalentUsage {
        input_tokens: (input_samples > 0).then_some(input_tokens).flatten(),
        cached_input_tokens: (input_samples > 0 && cached_samples == input_samples)
            .then_some(cached_input_tokens)
            .flatten(),
        cache_write_5m_tokens: cache_writes.then(|| cache_write_5m_tokens.unwrap_or_default()),
        cache_write_1h_tokens: cache_writes.then(|| cache_write_1h_tokens.unwrap_or_default()),
        unknown_cache_write_tokens: cache_writes
            .then(|| unknown_cache_write_tokens.unwrap_or_default()),
        output_tokens: (output_samples > 0).then_some(output_tokens).flatten(),
        total_tokens: (total_samples > 0).then_some(total_tokens).flatten(),
    })
}

fn usage_totals_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        requests: rust_u64(row.get(offset)?),
        successful_requests: rust_u64(row.get(offset + 1)?),
        latency_ms: rust_u64(row.get(offset + 2)?),
        ttft_ms: rust_u64(row.get(offset + 3)?),
        ttft_samples: rust_u64(row.get(offset + 4)?),
        generation_ms: rust_u64(row.get(offset + 5)?),
        generation_samples: rust_u64(row.get(offset + 6)?),
        generation_output_tokens: rust_u64(row.get(offset + 7)?),
        input_tokens: rust_u64(row.get(offset + 8)?),
        cached_input_tokens: rust_u64(row.get(offset + 9)?),
        cached_input_samples: rust_u64(row.get(offset + 10)?),
        cache_write_input_tokens: rust_u64(row.get(offset + 11)?),
        cache_write_input_samples: rust_u64(row.get(offset + 12)?),
        reasoning_tokens: rust_u64(row.get(offset + 13)?),
        output_tokens: rust_u64(row.get(offset + 14)?),
        total_tokens: rust_u64(row.get(offset + 15)?),
        speed_output_tokens: rust_u64(row.get(offset + 16)?),
        speed_duration_ms: rust_u64(row.get(offset + 17)?),
        api_equivalent: ApiEquivalentSummary::default(),
    })
}

pub(super) fn usage_log_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageLog> {
    let latency_ms: i64 = row.get(14)?;
    let ttft_ms: Option<i64> = row.get(15)?;
    let generation_ms: Option<i64> = row.get(16)?;
    let input_tokens: Option<i64> = row.get(17)?;
    let cached_input_tokens: Option<i64> = row.get(18)?;
    let cache_write_input_tokens: Option<i64> = row.get(19)?;
    let reasoning_tokens: Option<i64> = row.get(20)?;
    let output_tokens: Option<i64> = row.get(21)?;
    let total_tokens: Option<i64> = row.get(22)?;
    let service_tier: String = row.get(23)?;
    let applied_service_tier: Option<String> = row.get(24)?;
    let routing_json: Option<String> = row.get(25)?;
    let tool_use_json: Option<String> = row.get(26)?;
    let error_origin: Option<String> = row.get(27)?;
    let requested_reasoning_effort: Option<String> = row.get(28)?;
    let effective_reasoning_effort: Option<String> = row.get(29)?;
    let cache_write_ttl: Option<String> = row.get(30)?;
    let client_context_id: Option<String> = row.get(31)?;
    Ok(UsageLog {
        id: row.get(0)?,
        created_at: row.get(1)?,
        request_id: row.get(2)?,
        attempt: row.get(3)?,
        source_id: row.get(5)?,
        candidate_id: row.get(6)?,
        account_id: row.get(7)?,
        client_context_id,
        routing: routing_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        requested_model: row.get(8)?,
        resolved_model: row.get(9)?,
        requested_reasoning_effort: requested_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        effective_reasoning_effort: effective_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        wire_api: normalize_wire_api(row.get(10)?),
        service_tier: DefaultServiceTier::from_storage_value(&service_tier),
        applied_service_tier: applied_service_tier
            .as_deref()
            .and_then(zenith_relay_core::normalize_observed_service_tier),
        success: row.get(11)?,
        http_status: row.get(12)?,
        error_category: row.get(13)?,
        error_origin: error_origin.as_deref().and_then(|value| value.parse().ok()),
        tool_use: tool_use_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        latency_ms: rust_u64(latency_ms),
        ttft_ms: ttft_ms.map(rust_u64),
        generation_ms: generation_ms.map(rust_u64),
        input_tokens: input_tokens.map(rust_u64),
        cached_input_tokens: cached_input_tokens.map(rust_u64),
        cache_write_input_tokens: cache_write_input_tokens.map(rust_u64),
        cache_write_ttl: cache_write_ttl
            .as_deref()
            .and_then(zenith_relay_core::CacheWriteTtl::from_anthropic_ttl),
        reasoning_tokens: reasoning_tokens.map(rust_u64),
        output_tokens: output_tokens.map(rust_u64),
        total_tokens: total_tokens.map(rust_u64),
        api_equivalent: ApiEquivalentSummary::default(),
    })
}

fn normalize_wire_api(value: String) -> String {
    WireApi::from_storage_value(&value)
        .map(|wire_api| wire_api.as_str().to_string())
        .unwrap_or(value)
}

pub(super) fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(super) fn rust_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

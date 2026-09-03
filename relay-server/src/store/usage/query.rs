use super::super::sqlite::{db_error, optional_u64};
use super::UsagePriceResolver;
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use std::collections::HashMap;
use zenith_relay_core::{
    pricing::PriceSource,
    protocol::{UsageBucket, UsageGroup, UsageQuery, UsageTotals},
    sql_like_contains_pattern, ApiEquivalentSummary, ApiEquivalentUsage,
};

pub(super) const USAGE_TOTAL_COLUMNS: &str = "COUNT(*), \
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
        AND MAX(COALESCE(output_tokens, 0), 0) <= latency_ms \
        THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 \
        AND MAX(COALESCE(output_tokens, 0), 0) <= latency_ms \
        THEN latency_ms ELSE 0 END), 0)";

pub(super) fn usage_filter(query: &UsageQuery) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(value) = query.from_ms {
        clauses.push("created_at_ms >= ?");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.to_ms {
        clauses.push("created_at_ms <= ?");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.model_query.as_deref() {
        clauses.push("(requested_model LIKE ? ESCAPE '\\' OR resolved_model LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(sql_like_contains_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.source_or_account_query.as_deref() {
        clauses.push("candidate_hint LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(sql_like_contains_pattern(value)));
    }
    if let Some(value) = query.wire_api {
        clauses.push("wire_api = ?");
        values.push(SqlValue::Text(value.as_str().to_string()));
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
) -> Result<UsageTotals, String> {
    let sql = format!("SELECT {USAGE_TOTAL_COLUMNS} FROM usage_events{where_sql}");
    connection
        .query_row(&sql, params_from_iter(values.iter()), |row| {
            usage_totals_from_row(row, 0)
        })
        .map_err(db_error)
}

pub(super) fn usage_groups(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    key_sql: &str,
) -> Result<Vec<UsageGroup>, String> {
    let sql = format!(
        "SELECT {key_sql}, {USAGE_TOTAL_COLUMNS} FROM usage_events{where_sql} \
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
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

pub(super) fn usage_model_equivalents(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    resolver: &dyn UsagePriceResolver,
) -> Result<(HashMap<String, ApiEquivalentSummary>, Vec<PriceSource>), String> {
    let sql = format!(
        "SELECT candidate_kind, candidate_hint, COALESCE(resolved_model, requested_model, ''),
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
            SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END),
            SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END),
            SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h') THEN cache_write_input_tokens ELSE 0 END),
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens),
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens),
            COUNT(output_tokens), COUNT(total_tokens)
         FROM usage_events{where_sql} GROUP BY 1, 2, 3"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            let kind = row.get::<_, String>(0)?;
            let candidate_id = row.get::<_, String>(1)?;
            let model = row.get::<_, String>(2)?;
            let input_tokens = optional_u64(row.get(3)?);
            let cached_input_tokens = optional_u64(row.get(4)?);
            let cache_write_5m_tokens = optional_u64(row.get(6)?);
            let cache_write_1h_tokens = optional_u64(row.get(7)?);
            let unknown_cache_write_tokens = optional_u64(row.get(8)?);
            let output_tokens = optional_u64(row.get(9)?);
            let total_tokens = optional_u64(row.get(10)?);
            let input_samples = nonnegative_u64(row.get(11)?);
            let cached_samples = nonnegative_u64(row.get(12)?);
            let cache_write_samples = nonnegative_u64(row.get(13)?);
            let output_samples = nonnegative_u64(row.get(14)?);
            let total_samples = nonnegative_u64(row.get(15)?);
            let cache_writes = (cache_write_samples > 0).then_some(());
            let model_ref = (!model.is_empty()).then_some(model.as_str());
            let estimate = resolver.estimate(
                &kind,
                &candidate_id,
                model_ref,
                ApiEquivalentUsage {
                    input_tokens: (input_samples > 0).then_some(input_tokens).flatten(),
                    cached_input_tokens: (input_samples > 0 && cached_samples == input_samples)
                        .then_some(cached_input_tokens)
                        .flatten(),
                    cache_write_5m_tokens: cache_writes
                        .map(|_| cache_write_5m_tokens.unwrap_or_default()),
                    cache_write_1h_tokens: cache_writes
                        .map(|_| cache_write_1h_tokens.unwrap_or_default()),
                    unknown_cache_write_tokens: cache_writes
                        .map(|_| unknown_cache_write_tokens.unwrap_or_default()),
                    output_tokens: (output_samples > 0).then_some(output_tokens).flatten(),
                    total_tokens: (total_samples > 0).then_some(total_tokens).flatten(),
                },
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
) -> Result<Vec<UsageBucket>, String> {
    let Some(bucket_ms) = query.bucket_ms else {
        return Ok(Vec::new());
    };
    let start_ms = query.from_ms.unwrap_or_default();
    let start = SqlValue::Integer(start_ms.min(i64::MAX as u64) as i64);
    let bucket = SqlValue::Integer(bucket_ms.min(i64::MAX as u64) as i64);
    let bucket_sql = "? + ((created_at_ms - ?) / ?) * ?";
    let sql = format!(
        "SELECT {bucket_sql}, {USAGE_TOTAL_COLUMNS} \
         FROM usage_events{where_sql} GROUP BY 1 ORDER BY 1"
    );
    let mut parameters = vec![start.clone(), start, bucket.clone(), bucket];
    parameters.extend_from_slice(values);
    let mut buckets = {
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok(UsageBucket {
                    start_ms: nonnegative_u64(row.get(0)?),
                    totals: usage_totals_from_row(row, 1)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?
    };
    let price_sql = format!(
        "SELECT {bucket_sql}, candidate_kind, candidate_hint, \
            COALESCE(resolved_model, requested_model), \
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens), \
            SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), \
            SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), \
            SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h') THEN cache_write_input_tokens ELSE 0 END), \
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens), \
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens), \
            COUNT(output_tokens), COUNT(total_tokens) \
         FROM usage_events{where_sql} GROUP BY 1, 2, 3, 4"
    );
    let mut statement = connection.prepare(&price_sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            let kind = row.get::<_, String>(1)?;
            let candidate_id = row.get::<_, String>(2)?;
            let model = row.get::<_, Option<String>>(3)?;
            let input_tokens: Option<i64> = row.get(4)?;
            let cached_input_tokens: Option<i64> = row.get(5)?;
            let cache_write_5m_tokens: Option<i64> = row.get(7)?;
            let cache_write_1h_tokens: Option<i64> = row.get(8)?;
            let unknown_cache_write_tokens: Option<i64> = row.get(9)?;
            let output_tokens: Option<i64> = row.get(10)?;
            let total_tokens: Option<i64> = row.get(11)?;
            let input_samples: i64 = row.get(12)?;
            let cached_samples: i64 = row.get(13)?;
            let cache_write_samples: i64 = row.get(14)?;
            let output_samples: i64 = row.get(15)?;
            let total_samples: i64 = row.get(16)?;
            let start_ms = nonnegative_u64(row.get(0)?);
            let input_tokens = (input_samples > 0)
                .then(|| optional_u64(input_tokens))
                .flatten();
            let cached_input_tokens = (input_samples > 0 && cached_samples == input_samples)
                .then(|| optional_u64(cached_input_tokens))
                .flatten();
            let cache_writes = (cache_write_samples > 0).then_some(());
            Ok((
                start_ms,
                resolver.estimate(
                    &kind,
                    &candidate_id,
                    model.as_deref(),
                    ApiEquivalentUsage {
                        input_tokens,
                        cached_input_tokens,
                        cache_write_5m_tokens: cache_writes
                            .map(|_| optional_u64(cache_write_5m_tokens).unwrap_or_default()),
                        cache_write_1h_tokens: cache_writes
                            .map(|_| optional_u64(cache_write_1h_tokens).unwrap_or_default()),
                        unknown_cache_write_tokens: cache_writes
                            .map(|_| optional_u64(unknown_cache_write_tokens).unwrap_or_default()),
                        output_tokens: (output_samples > 0)
                            .then(|| optional_u64(output_tokens))
                            .flatten(),
                        total_tokens: (total_samples > 0)
                            .then(|| optional_u64(total_tokens))
                            .flatten(),
                    },
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

fn usage_totals_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        requests: nonnegative_u64(row.get(offset)?),
        successful_requests: nonnegative_u64(row.get(offset + 1)?),
        latency_ms: nonnegative_u64(row.get(offset + 2)?),
        ttft_ms: nonnegative_u64(row.get(offset + 3)?),
        ttft_samples: nonnegative_u64(row.get(offset + 4)?),
        generation_ms: nonnegative_u64(row.get(offset + 5)?),
        generation_samples: nonnegative_u64(row.get(offset + 6)?),
        generation_output_tokens: nonnegative_u64(row.get(offset + 7)?),
        input_tokens: nonnegative_u64(row.get(offset + 8)?),
        cached_input_tokens: nonnegative_u64(row.get(offset + 9)?),
        cached_input_samples: nonnegative_u64(row.get(offset + 10)?),
        cache_write_input_tokens: nonnegative_u64(row.get(offset + 11)?),
        cache_write_input_samples: nonnegative_u64(row.get(offset + 12)?),
        reasoning_tokens: nonnegative_u64(row.get(offset + 13)?),
        output_tokens: nonnegative_u64(row.get(offset + 14)?),
        total_tokens: nonnegative_u64(row.get(offset + 15)?),
        speed_output_tokens: nonnegative_u64(row.get(offset + 16)?),
        speed_duration_ms: nonnegative_u64(row.get(offset + 17)?),
        api_equivalent: ApiEquivalentSummary::default(),
    })
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

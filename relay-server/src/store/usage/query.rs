use super::super::{
    configuration::SourcePriceOverrides,
    sqlite::{db_error, optional_u64},
};
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use std::collections::{BTreeMap, HashMap};
use zenith_relay_core::{
    estimate_api_equivalent_with_price_override,
    protocol::{UsageBucket, UsageGroup, UsageQuery, UsageTotals},
    ApiEquivalentSummary, ApiModelPriceOverride, DefaultServiceTier, WireApi,
};

pub(super) const USAGE_TOTAL_COLUMNS: &str = "COUNT(*), \
    COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
    COALESCE(SUM(latency_ms), 0), COALESCE(SUM(ttft_ms), 0), COUNT(ttft_ms), \
    COALESCE(SUM(CASE WHEN success != 0 THEN generation_ms ELSE 0 END), 0), \
    COUNT(CASE WHEN success != 0 THEN generation_ms END), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms IS NOT NULL \
        THEN MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0), 0) ELSE 0 END), 0), \
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0), \
    COUNT(cached_input_tokens), COALESCE(SUM(cache_write_input_tokens), 0), \
    COUNT(cache_write_input_tokens), COALESCE(SUM(reasoning_tokens), 0), \
    COALESCE(SUM(output_tokens), 0), \
    COALESCE(SUM(total_tokens), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > COALESCE(reasoning_tokens, 0) \
        THEN output_tokens - COALESCE(reasoning_tokens, 0) ELSE 0 END), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > COALESCE(reasoning_tokens, 0) AND latency_ms > 0 \
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
        let value = SqlValue::Text(like_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.source_or_account_query.as_deref() {
        clauses.push("candidate_hint LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
    }
    if let Some(value) = query.wire_api {
        clauses.push("wire_api = ?");
        values.push(SqlValue::Text(wire_api_name(value).to_string()));
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
        values.push(SqlValue::Text(like_pattern(value)));
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
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
    let sql = format!(
        "SELECT candidate_kind, candidate_hint, COALESCE(resolved_model, requested_model, ''),
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
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
            let cache_write_input_tokens = optional_u64(row.get(5)?);
            let output_tokens = optional_u64(row.get(6)?);
            let total_tokens = optional_u64(row.get(7)?);
            let input_samples = nonnegative_u64(row.get(8)?);
            let cached_samples = nonnegative_u64(row.get(9)?);
            let cache_write_samples = nonnegative_u64(row.get(10)?);
            let output_samples = nonnegative_u64(row.get(11)?);
            let total_samples = nonnegative_u64(row.get(12)?);
            Ok((
                model.clone(),
                estimate_api_equivalent_with_price_override(
                    (!model.is_empty()).then_some(model.as_str()),
                    (input_samples > 0).then_some(input_tokens).flatten(),
                    (input_samples > 0 && cached_samples == input_samples)
                        .then_some(cached_input_tokens)
                        .flatten(),
                    (input_samples > 0 && cache_write_samples == input_samples)
                        .then_some(cache_write_input_tokens)
                        .flatten(),
                    (output_samples > 0).then_some(output_tokens).flatten(),
                    (total_samples > 0).then_some(total_tokens).flatten(),
                    configured_model_price(
                        price_overrides,
                        source_price_overrides,
                        &kind,
                        &candidate_id,
                        (!model.is_empty()).then_some(model.as_str()),
                    ),
                ),
            ))
        })
        .map_err(db_error)?;
    let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
    for row in rows {
        let (model, estimate) = row.map_err(db_error)?;
        equivalents.entry(model).or_default().merge(estimate);
    }
    Ok(equivalents)
}

pub(super) fn usage_buckets(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    query: &UsageQuery,
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
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
            let cache_write_input_tokens: Option<i64> = row.get(6)?;
            let output_tokens: Option<i64> = row.get(7)?;
            let total_tokens: Option<i64> = row.get(8)?;
            let input_samples: i64 = row.get(9)?;
            let cached_samples: i64 = row.get(10)?;
            let cache_write_samples: i64 = row.get(11)?;
            let output_samples: i64 = row.get(12)?;
            let total_samples: i64 = row.get(13)?;
            let start_ms = nonnegative_u64(row.get(0)?);
            let input_tokens = (input_samples > 0)
                .then(|| optional_u64(input_tokens))
                .flatten();
            let cached_input_tokens = (input_samples > 0 && cached_samples == input_samples)
                .then(|| optional_u64(cached_input_tokens))
                .flatten();
            let cache_write_input_tokens = (input_samples > 0
                && cache_write_samples == input_samples)
                .then(|| optional_u64(cache_write_input_tokens))
                .flatten();
            Ok((
                start_ms,
                estimate_api_equivalent_with_price_override(
                    model.as_deref(),
                    input_tokens,
                    cached_input_tokens,
                    cache_write_input_tokens,
                    (output_samples > 0)
                        .then(|| optional_u64(output_tokens))
                        .flatten(),
                    (total_samples > 0)
                        .then(|| optional_u64(total_tokens))
                        .flatten(),
                    configured_model_price(
                        price_overrides,
                        source_price_overrides,
                        &kind,
                        &candidate_id,
                        model.as_deref(),
                    ),
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

pub(super) fn service_tier_name(value: DefaultServiceTier) -> &'static str {
    match value {
        DefaultServiceTier::Standard => "standard",
        DefaultServiceTier::Fast => "fast",
    }
}

pub(super) fn parse_service_tier(value: &str) -> DefaultServiceTier {
    if value.eq_ignore_ascii_case("fast") || value.eq_ignore_ascii_case("priority") {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

pub(super) fn wire_api_name(value: WireApi) -> &'static str {
    match value {
        WireApi::Responses => "responses",
        WireApi::ChatCompletions => "chat_completions",
        WireApi::Messages => "messages",
    }
}

pub(super) fn parse_wire_api(value: &str) -> WireApi {
    match value {
        "chat_completions" => WireApi::ChatCompletions,
        "messages" => WireApi::Messages,
        _ => WireApi::Responses,
    }
}

pub(super) fn configured_model_price(
    overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_overrides: &SourcePriceOverrides,
    candidate_kind: &str,
    candidate_id: &str,
    model: Option<&str>,
) -> Option<ApiModelPriceOverride> {
    let model = model?.to_ascii_lowercase();
    (candidate_kind == "source")
        .then(|| source_overrides.get(candidate_id)?.get(&model).copied())
        .flatten()
        .or_else(|| overrides.get(&model).copied())
}

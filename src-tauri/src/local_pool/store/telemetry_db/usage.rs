use super::{db_error, SourcePriceOverrides, UsageLog};
use crate::local_pool::error::Result;
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use std::collections::{BTreeMap, HashMap};
use zenith_relay_core::{
    estimate_api_equivalent_with_price_override,
    protocol::{UsageBucket, UsageGroup, UsageQuery, UsageTotals},
    sql_like_contains_pattern, ApiEquivalentSummary, ApiModelPriceOverride, DefaultServiceTier,
    WireApi,
};

const USAGE_TOTAL_COLUMNS: &str = "COUNT(*), \
    COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
    COALESCE(SUM(latency_ms), 0), COALESCE(SUM(ttft_ms), 0), COUNT(ttft_ms), \
    COALESCE(SUM(CASE WHEN success != 0 THEN generation_ms ELSE 0 END), 0), \
    COUNT(CASE WHEN success != 0 THEN generation_ms END), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms IS NOT NULL \
        THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), \
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0), \
    COUNT(cached_input_tokens), COALESCE(SUM(cache_write_input_tokens), 0), \
    COUNT(cache_write_input_tokens), COALESCE(SUM(reasoning_tokens), 0), \
    COALESCE(SUM(output_tokens), 0), \
    COALESCE(SUM(total_tokens), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 \
        THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 \
        THEN latency_ms ELSE 0 END), 0)";

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
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> Result<HashMap<String, ApiEquivalentSummary>> {
    let sql = format!(
        "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
            COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens),
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens),
            COUNT(output_tokens), COUNT(total_tokens)
         FROM request_logs{where_sql} GROUP BY 1, 2, 3"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            let kind = row.get::<_, String>(0)?;
            let candidate_id = row.get::<_, String>(1)?;
            let model = row.get::<_, String>(2)?;
            let input_tokens = row.get::<_, Option<i64>>(3)?.map(rust_u64);
            let cached_input_tokens = row.get::<_, Option<i64>>(4)?.map(rust_u64);
            let cache_write_input_tokens = row.get::<_, Option<i64>>(5)?.map(rust_u64);
            let output_tokens = row.get::<_, Option<i64>>(6)?.map(rust_u64);
            let total_tokens = row.get::<_, Option<i64>>(7)?.map(rust_u64);
            let input_samples = rust_u64(row.get(8)?);
            let cached_samples = rust_u64(row.get(9)?);
            let cache_write_samples = rust_u64(row.get(10)?);
            let output_samples = rust_u64(row.get(11)?);
            let total_samples = rust_u64(row.get(12)?);
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
            let start_ms = rust_u64(row.get(0)?);
            let input_tokens = (input_samples > 0)
                .then(|| input_tokens.map(rust_u64))
                .flatten();
            let cached_input_tokens = (input_samples > 0 && cached_samples == input_samples)
                .then(|| cached_input_tokens.map(rust_u64))
                .flatten();
            let cache_write_input_tokens = (input_samples > 0
                && cache_write_samples == input_samples)
                .then(|| cache_write_input_tokens.map(rust_u64))
                .flatten();
            Ok((
                start_ms,
                estimate_api_equivalent_with_price_override(
                    model.as_deref(),
                    input_tokens,
                    cached_input_tokens,
                    cache_write_input_tokens,
                    (output_samples > 0)
                        .then(|| output_tokens.map(rust_u64))
                        .flatten(),
                    (total_samples > 0)
                        .then(|| total_tokens.map(rust_u64))
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

pub(super) fn configured_model_price(
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
    candidate_kind: &str,
    candidate_id: &str,
    model: Option<&str>,
) -> Option<ApiModelPriceOverride> {
    let model = model?.to_ascii_lowercase();
    (candidate_kind == "source")
        .then(|| {
            source_price_overrides
                .get(candidate_id)?
                .get(&model)
                .copied()
        })
        .flatten()
        .or_else(|| price_overrides.get(&model).copied())
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
    Ok(UsageLog {
        id: row.get(0)?,
        created_at: row.get(1)?,
        request_id: row.get(2)?,
        attempt: row.get(3)?,
        source_id: row.get(5)?,
        candidate_id: row.get(6)?,
        account_id: row.get(7)?,
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
            .map(DefaultServiceTier::from_storage_value),
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
    if value == "chatcompletions" {
        "chat_completions".to_string()
    } else {
        value
    }
}

pub(super) fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(super) fn rust_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

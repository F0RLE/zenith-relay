use super::*;
use std::collections::BTreeMap;
use zenith_relay_core::protocol::UsageBucket;

pub(in crate::local_pool::store::telemetry_db) fn usage_buckets(
    connection: &Connection,
    where_sql: &str,
    query_parameters: &[SqlValue],
    query: &UsageQuery,
    resolver: &CatalogPriceResolver<'_>,
) -> Result<Vec<UsageBucket>> {
    let Some(bucket_ms) = query.bucket_ms else {
        return Ok(Vec::new());
    };
    let start = SqlValue::Integer(sql_u64(query.from_ms.unwrap_or_default()));
    let bucket = SqlValue::Integer(sql_u64(bucket_ms));
    let bucket_sql = "? + ((CAST(strftime('%s', created_at) AS INTEGER) * 1000 - ?) / ?) * ?";
    let mut parameters = vec![start.clone(), start, bucket.clone(), bucket];
    parameters.extend_from_slice(query_parameters);
    let mut statement = connection
        .prepare(&format!(
            "SELECT {bucket_sql}, {USAGE_REQUEST_COLUMNS}
             FROM request_logs{where_sql} GROUP BY 1"
        ))
        .map_err(db_error)?;
    let mut requests = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok((rust_u64(row.get(0)?), usage_totals_from_row(row, 1)?))
        })
        .map_err(db_error)?
        .collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(db_error)?;
    drop(statement);

    // Consumption and price share the physical scan, but keep pricing grouped
    // by bucket/member/model/tier/context before rounding each estimate.
    let sends = send_projection(
        &format!("created_at, reasoning_tokens, {USAGE_PRICING_INPUT_COLUMNS}"),
        where_sql,
    );
    let mut statement = connection
        .prepare(&format!(
            "SELECT {bucket_sql}, CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
             COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model),
             {price_class}, {context_band}, {USAGE_PRICING_AGGREGATE_COLUMNS},
             0 AS totals_start, {USAGE_CONSUMPTION_COLUMNS}
             FROM ({sends}) GROUP BY 1, 2, 3, 4, 5, 6",
            price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
            context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL,
        ))
        .map_err(db_error)?;
    let totals_offset = statement.column_index("totals_start").map_err(db_error)? + 1;
    parameters.extend_from_slice(query_parameters);
    let mut rows = statement
        .query(params_from_iter(parameters.iter()))
        .map_err(db_error)?;
    let mut buckets = BTreeMap::<u64, UsageTotals>::new();
    while let Some(row) = rows.next().map_err(db_error)? {
        let start_ms = rust_u64(row.get(0).map_err(db_error)?);
        let kind: String = row.get(1).map_err(db_error)?;
        let member: String = row.get(2).map_err(db_error)?;
        let model: Option<String> = row.get(3).map_err(db_error)?;
        let totals = buckets.entry(start_ms).or_default();
        apply_usage_totals_delta(
            totals,
            usage_totals_from_row(row, totals_offset).map_err(db_error)?,
            true,
        );
        totals.api_equivalent.merge(resolver.estimate(
            &kind,
            &member,
            model.as_deref(),
            usage_pricing_usage_from_row(row, 6).map_err(db_error)?,
        ));
    }
    Ok(buckets
        .into_iter()
        .map(|(start_ms, totals)| UsageBucket {
            start_ms,
            totals: report::priced_totals(requests.remove(&start_ms).unwrap_or_default(), totals),
        })
        .collect())
}

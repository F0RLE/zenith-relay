use super::db_error;
use super::CatalogPriceResolver;
use crate::local_pool::error::Result;
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use std::collections::HashMap;
use zenith_relay_core::{
    pricing::PriceSource,
    protocol::{
        assign_bucket_equivalents, merge_model_equivalents, UsageBucket, UsageGroup, UsageQuery,
        UsageTotals,
    },
    ApiEquivalentSummary, ApiEquivalentUsage,
};

mod rows;
mod totals;

pub(super) use rows::{rust_u64, sql_u64, usage_log_from_row};
use rows::{usage_pricing_usage_from_row, usage_totals_from_row};
pub(super) use totals::{
    apply_usage_totals_delta, is_unfiltered_all_time, usage_filter, usage_totals_from_event,
    usage_totals_from_sample, UsageTotalsSample,
};

const USAGE_TOTAL_COLUMNS: &str =
    zenith_relay_core::usage_total_columns_sql!("COALESCE(output_tokens, 0) <= latency_ms");

/// Aggregate columns used when only API-equivalent pricing is needed. The
/// sample counts are important: a cache value is only treated as complete
/// when every row in the aggregate reported that measurement.
const USAGE_PRICING_AGGREGATE_COLUMNS: &str =
    zenith_relay_core::usage::API_EQUIVALENT_AGGREGATE_SQL;

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
        SqlValue::Integer(sql_u64(from_ms)),
        SqlValue::Integer(sql_u64(to_ms)),
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
    resolver: &CatalogPriceResolver<'_>,
    use_rollup: bool,
) -> Result<(HashMap<String, ApiEquivalentSummary>, Vec<PriceSource>)> {
    let sql = if use_rollup && where_sql.is_empty() {
        // Current rows are maintained transactionally in the rollup. Keep a
        // small compatibility branch for rows written by an older Relay
        // version (or a recovery tool) before the aggregate flag existed.
        format!(
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
                {USAGE_PRICING_AGGREGATE_COLUMNS}
             FROM request_logs
             WHERE usage_aggregate_recorded = 0
             GROUP BY 1, 2, 3"
        )
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
    let rows = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(merge_model_equivalents(rows))
}

pub(super) fn usage_buckets(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    query: &UsageQuery,
    resolver: &CatalogPriceResolver<'_>,
) -> Result<Vec<UsageBucket>> {
    let Some(bucket_ms) = query.bucket_ms else {
        return Ok(Vec::new());
    };
    let start_ms = query.from_ms.unwrap_or_default();
    let start = SqlValue::Integer(sql_u64(start_ms));
    let bucket = SqlValue::Integer(sql_u64(bucket_ms));
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
            {USAGE_PRICING_AGGREGATE_COLUMNS} \
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
    let rows = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)?;
    assign_bucket_equivalents(&mut buckets, rows);
    Ok(buckets)
}

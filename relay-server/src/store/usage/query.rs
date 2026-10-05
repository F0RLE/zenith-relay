use super::super::sqlite::db_error;
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use std::collections::HashMap;
use zenith_relay_core::usage::sql_count_u64;
use zenith_relay_core::CatalogPriceResolver;
use zenith_relay_core::{
    pricing::PriceSource,
    protocol::{
        assign_bucket_equivalents, merge_model_equivalents, UsageBucket, UsageGroup, UsageQuery,
        UsageTotals,
    },
    sql_like_contains_pattern, ApiEquivalentSummary, ApiEquivalentUsage, ObservedUsageSums,
};

pub(super) const USAGE_TOTAL_COLUMNS: &str =
    zenith_relay_core::usage_total_columns_sql!("MAX(COALESCE(output_tokens, 0), 0) <= latency_ms");

const USAGE_PRICING_AGGREGATE_COLUMNS: &str =
    zenith_relay_core::usage::API_EQUIVALENT_AGGREGATE_SQL;

pub(super) fn usage_filter(query: &UsageQuery) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(value) = query.from_ms {
        clauses.push("created_at_ms >= ?");
        values.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(value)));
    }
    if let Some(value) = query.to_ms {
        clauses.push("created_at_ms <= ?");
        values.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(value)));
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
    if let Some(value) = query.transport {
        clauses.push("transport = ?");
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
    resolver: &CatalogPriceResolver<'_>,
) -> Result<(HashMap<String, ApiEquivalentSummary>, Vec<PriceSource>), String> {
    let sql = format!(
        "SELECT candidate_kind, candidate_hint, COALESCE(resolved_model, requested_model, ''),
            {price_class}, {context_band},
            {USAGE_PRICING_AGGREGATE_COLUMNS}
         FROM usage_events{where_sql} GROUP BY 1, 2, 3, 4, 5",
        price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
        context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            let kind = row.get::<_, String>(0)?;
            let candidate_id = row.get::<_, String>(1)?;
            let model = row.get::<_, String>(2)?;
            let usage = aggregate_usage_from_row(row, 5)?;
            let model_ref = (!model.is_empty()).then_some(model.as_str());
            let estimate = resolver.estimate(&kind, &candidate_id, model_ref, usage);
            let source = resolver.source(&kind, &candidate_id, model_ref);
            Ok((model.clone(), estimate, source))
        })
        .map_err(db_error)?;
    let rows = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
    Ok(merge_model_equivalents(rows))
}

pub(super) fn candidate_window_usage(
    connection: &Connection,
    candidate_hint: &str,
    from_ms: u64,
    to_ms: u64,
) -> Result<Vec<(String, ApiEquivalentUsage)>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT COALESCE(resolved_model, requested_model, ''),
                    {price_class}, {context_band},
                    {USAGE_PRICING_AGGREGATE_COLUMNS}
                 FROM usage_events
                 WHERE candidate_kind = 'account' AND candidate_hint = ?1
                   AND created_at_ms >= ?2 AND created_at_ms <= ?3
                 GROUP BY 1, 2, 3",
            price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
            context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL
        ))
        .map_err(db_error)?;
    let values = [
        SqlValue::Text(candidate_hint.to_string()),
        SqlValue::Integer(zenith_relay_core::usage::sql_u64(from_ms)),
        SqlValue::Integer(zenith_relay_core::usage::sql_u64(to_ms)),
    ];
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            let model = row.get::<_, String>(0)?;
            Ok((model, aggregate_usage_from_row(row, 3)?))
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

fn aggregate_usage_from_row(
    row: &rusqlite::Row<'_>,
    start: usize,
) -> rusqlite::Result<ApiEquivalentUsage> {
    let price_class: String = row.get(start - 2)?;
    let context_band: String = row.get(start - 1)?;
    Ok(
        ApiEquivalentUsage::from_observed_sums(ObservedUsageSums::from_priced_aggregate(
            |column| row.get(start + column),
            |column| row.get(start + column),
        )?)
        .with_aggregate_rates(&price_class, &context_band),
    )
}

pub(super) fn usage_buckets(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    query: &UsageQuery,
    resolver: &CatalogPriceResolver<'_>,
) -> Result<Vec<UsageBucket>, String> {
    let Some(bucket_ms) = query.bucket_ms else {
        return Ok(Vec::new());
    };
    let start_ms = query.from_ms.unwrap_or_default();
    let start = SqlValue::Integer(zenith_relay_core::usage::sql_u64(start_ms));
    let bucket = SqlValue::Integer(zenith_relay_core::usage::sql_u64(bucket_ms));
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
                    start_ms: sql_count_u64(row.get(0)?),
                    totals: usage_totals_from_row(row, 1)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?
    };
    let price_sql = format!(
        "SELECT {bucket_sql}, candidate_kind, candidate_hint, \
            COALESCE(resolved_model, requested_model), \
            {price_class}, {context_band}, \
            {USAGE_PRICING_AGGREGATE_COLUMNS} \
         FROM usage_events{where_sql} GROUP BY 1, 2, 3, 4, 5, 6",
        price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
        context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL
    );
    let mut statement = connection.prepare(&price_sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            let kind = row.get::<_, String>(1)?;
            let candidate_id = row.get::<_, String>(2)?;
            let model = row.get::<_, Option<String>>(3)?;
            let start_ms = sql_count_u64(row.get(0)?);
            Ok((
                start_ms,
                resolver.estimate(
                    &kind,
                    &candidate_id,
                    model.as_deref(),
                    aggregate_usage_from_row(row, 6)?,
                ),
            ))
        })
        .map_err(db_error)?;
    let rows = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
    assign_bucket_equivalents(&mut buckets, rows);
    Ok(buckets)
}

fn usage_totals_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<UsageTotals> {
    UsageTotals::from_sql_counts(|column| row.get(offset + column))
}

use super::CatalogPriceResolver;
use super::db_error;
use crate::local_pool::error::Result;
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use std::collections::HashMap;
use zenith_relay_core::{
    ApiEquivalentSummary,
    pricing::PriceSource,
    protocol::{UsageGroup, UsageQuery, UsageTotals, merge_model_equivalents},
};

mod buckets;
mod filters;
mod report;
mod rows;
mod sends;
mod totals;

pub(super) use buckets::usage_buckets;
pub(super) use report::scoped_report_summary;
pub(super) use rows::{rust_u64, sql_u64, usage_log_from_row};
use rows::{usage_pricing_usage_from_row, usage_totals_from_row};
pub(super) use sends::{LEGACY_SEND_PREDICATE, send_projection};
use totals::combine_totals;
pub(super) use totals::{
    UsageTotalsSample, apply_usage_totals_delta, consumption_totals, is_unfiltered_all_time,
    request_totals, usage_filter, usage_totals_from_event, usage_totals_from_sample,
};

const USAGE_REQUEST_COLUMNS: &str = zenith_relay_core::usage_total_columns_sql!(logical);
const USAGE_CONSUMPTION_COLUMNS: &str = zenith_relay_core::usage_total_columns_sql!(physical);

// Project before the UNION, so SQLite does not carry diagnostic JSON through
// report GROUP BYs. Filters still run on the original rows in both branches.
const USAGE_CONSUMPTION_INPUT_COLUMNS: &str = "input_tokens, cached_input_tokens,
     cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens";
const USAGE_PRICING_INPUT_COLUMNS: &str =
    "account_id, source_id, requested_model, resolved_model, applied_service_tier,
     input_tokens, cached_input_tokens, cache_write_input_tokens, cache_write_ttl,
     output_tokens, total_tokens";

/// Aggregate columns used when only API-equivalent pricing is needed. The
/// sample counts are important: a cache value is only treated as complete
/// when every row in the aggregate reported that measurement.
const USAGE_PRICING_AGGREGATE_COLUMNS: &str =
    zenith_relay_core::usage::API_EQUIVALENT_AGGREGATE_SQL;

pub(super) fn usage_totals(
    connection: &Connection,
    where_sql: &str,
    query_parameters: &[SqlValue],
) -> Result<UsageTotals> {
    let requests = connection
        .query_row(
            &format!("SELECT {USAGE_REQUEST_COLUMNS} FROM request_logs{where_sql}"),
            params_from_iter(query_parameters.iter()),
            |row| usage_totals_from_row(row, 0),
        )
        .map_err(db_error)?;
    let sends = send_projection(USAGE_CONSUMPTION_INPUT_COLUMNS, where_sql);
    let consumption = connection
        .query_row(
            &format!("SELECT {USAGE_CONSUMPTION_COLUMNS} FROM ({sends})"),
            params_from_iter(query_parameters.iter().chain(query_parameters.iter())),
            |row| usage_totals_from_row(row, 0),
        )
        .map_err(db_error)?;
    Ok(combine_totals(requests, consumption))
}

pub(super) fn usage_groups(
    connection: &Connection,
    where_sql: &str,
    query_parameters: &[SqlValue],
    key_sql: &str,
) -> Result<Vec<UsageGroup>> {
    let read = |sql: &str, parameters: &[SqlValue]| -> Result<Vec<UsageGroup>> {
        let mut statement = connection.prepare(sql).map_err(db_error)?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok(UsageGroup {
                    key: row.get(0)?,
                    label: None,
                    totals: usage_totals_from_row(row, 1)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)
    };
    let mut requests: HashMap<_, _> = read(
        &format!(
            "SELECT {key_sql}, {USAGE_REQUEST_COLUMNS} FROM request_logs{where_sql} GROUP BY 1"
        ),
        query_parameters,
    )?
    .into_iter()
    .map(|group| (group.key, group.totals))
    .collect();
    let sends = send_projection(
        &format!("{key_sql} AS group_key, {USAGE_CONSUMPTION_INPUT_COLUMNS}"),
        where_sql,
    );
    let send_parameters: Vec<_> = query_parameters
        .iter()
        .chain(query_parameters.iter())
        .cloned()
        .collect();
    let mut groups = read(
        &format!("SELECT group_key, {USAGE_CONSUMPTION_COLUMNS} FROM ({sends}) GROUP BY 1"),
        &send_parameters,
    )?;
    for group in &mut groups {
        group.totals = combine_totals(
            requests.remove(&group.key).unwrap_or_default(),
            group.totals.clone(),
        );
    }
    groups.sort_by(|a, b| {
        b.totals
            .requests
            .cmp(&a.totals.requests)
            .then(a.key.cmp(&b.key))
    });
    Ok(groups)
}

pub(super) struct GroupEquivalents {
    pub models: HashMap<String, ApiEquivalentSummary>,
    pub members: HashMap<String, ApiEquivalentSummary>,
    pub sources: Vec<PriceSource>,
}

pub(super) fn usage_model_equivalents(
    connection: &Connection,
    where_sql: &str,
    query_parameters: &[SqlValue],
    resolver: &CatalogPriceResolver<'_>,
    use_rollup: bool,
) -> Result<GroupEquivalents> {
    let sql = if use_rollup && where_sql.is_empty() {
        // Current rows are maintained transactionally in the rollup. Keep a
        // small compatibility branch for rows written by an older Relay
        // version (or a recovery tool) before the aggregate flag existed.
        format!(
            "SELECT candidate_kind, candidate_id, model,
                price_class, context_band,
                input_tokens, cached_input_tokens, cache_write_input_tokens,
                cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
                output_tokens, total_tokens, input_samples,
                cached_input_samples, cache_write_input_samples,
                output_samples, total_samples
             FROM usage_candidate_rollups
             UNION ALL
             SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                {price_class}, {context_band},
                {USAGE_PRICING_AGGREGATE_COLUMNS}
             FROM usage_sends
             WHERE usage_aggregate_recorded = 0
             GROUP BY 1, 2, 3, 4, 5",
            price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
            context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL
        )
    } else {
        let sends = send_projection(USAGE_PRICING_INPUT_COLUMNS, where_sql);
        format!(
            "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                {price_class}, {context_band},
                {USAGE_PRICING_AGGREGATE_COLUMNS}
             FROM ({sends}) GROUP BY 1, 2, 3, 4, 5",
            price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
            context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL
        )
    };
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(
            params_from_iter(query_parameters.iter().chain(query_parameters.iter())),
            |row| {
                let kind = row.get::<_, String>(0)?;
                let candidate_id = row.get::<_, String>(1)?;
                let model_id = row.get::<_, String>(2)?;
                let model_ref = (!model_id.is_empty()).then_some(model_id.as_str());
                let estimate = resolver.estimate(
                    &kind,
                    &candidate_id,
                    model_ref,
                    usage_pricing_usage_from_row(row, 5)?,
                );
                let price_source = resolver.source(&kind, &candidate_id, model_ref);
                Ok((candidate_id, model_id, estimate, price_source))
            },
        )
        .map_err(db_error)?;
    let mut members = HashMap::<String, ApiEquivalentSummary>::new();
    let mut model_rows = Vec::new();
    for row in rows {
        let (member, model, estimate, source) = row.map_err(db_error)?;
        members.entry(member).or_default().merge(estimate);
        model_rows.push((model, estimate, source));
    }
    let (models, sources) = merge_model_equivalents(model_rows);
    Ok(GroupEquivalents {
        models,
        members,
        sources,
    })
}

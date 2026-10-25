use super::*;
use crate::local_pool::store::telemetry_db::{reader::ReportRead, TelemetryDb};

pub(in crate::local_pool::store::telemetry_db) struct ReportSummary {
    pub totals: UsageTotals,
    pub models: Vec<UsageGroup>,
    pub members: Vec<UsageGroup>,
    pub sources: Vec<PriceSource>,
    pub sends: u64,
}

#[derive(Default)]
struct Projection {
    totals: UsageTotals,
    models: HashMap<String, UsageTotals>,
    members: HashMap<String, UsageTotals>,
}

fn add(target: &mut UsageTotals, totals: &UsageTotals) {
    apply_usage_totals_delta(target, totals.clone(), true);
    target.api_equivalent.merge(totals.api_equivalent);
}

impl Projection {
    fn add(&mut self, query: &UsageQuery, model: &str, member: &str, totals: UsageTotals) {
        add(&mut self.totals, &totals);
        if query.includes_models() {
            add(self.models.entry(model.to_string()).or_default(), &totals);
        }
        if query.includes_pool_members() {
            add(self.members.entry(member.to_string()).or_default(), &totals);
        }
    }
}

fn priced_totals(requests: UsageTotals, sends: UsageTotals) -> UsageTotals {
    let price = sends.api_equivalent;
    UsageTotals {
        api_equivalent: price,
        ..combine_totals(requests, sends)
    }
}

fn groups(
    mut requests: HashMap<String, UsageTotals>,
    sends: HashMap<String, UsageTotals>,
) -> Vec<UsageGroup> {
    let mut groups = sends
        .into_iter()
        .map(|(key, totals)| UsageGroup {
            totals: priced_totals(requests.remove(&key).unwrap_or_default(), totals),
            key,
            label: None,
        })
        .collect::<Vec<_>>();
    groups.sort_by(|a, b| {
        b.totals
            .requests
            .cmp(&a.totals.requests)
            .then(a.key.cmp(&b.key))
    });
    groups
}

/// Two lean scans replace separate totals, model, member, pricing and count
/// scans. Keep logical timing/counts separate from physical-send consumption.
/// Pricing uses the original member/model/tier/context grouping: splitting it
/// into time buckets here would change integer price rounding.
pub(in crate::local_pool::store::telemetry_db) fn scoped_report_summary(
    connection: &Connection,
    query: &UsageQuery,
    where_sql: &str,
    parameters: &[SqlValue],
    resolver: &CatalogPriceResolver<'_>,
) -> Result<ReportSummary> {
    let mut requests = Projection::default();
    let mut statement = connection
        .prepare(&format!(
        "SELECT COALESCE(resolved_model, requested_model, ''), COALESCE(account_id, source_id, ''),
         {USAGE_TOTAL_COLUMNS} FROM request_logs{where_sql} GROUP BY 1, 2"
    ))
        .map_err(db_error)?;
    let mut rows = statement
        .query(params_from_iter(parameters.iter()))
        .map_err(db_error)?;
    while let Some(row) = rows.next().map_err(db_error)? {
        let model: String = row.get(0).map_err(db_error)?;
        let member: String = row.get(1).map_err(db_error)?;
        requests.add(
            query,
            &model,
            &member,
            usage_totals_from_row(row, 2).map_err(db_error)?,
        );
    }
    drop(rows);
    drop(statement);

    let projection = send_projection(&format!(
        "account_id, source_id, requested_model, resolved_model, applied_service_tier, cache_write_ttl,
         {USAGE_TOTAL_INPUT_COLUMNS}"
    ), where_sql);
    let mut statement = connection
        .prepare(&format!(
            "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
         COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
         {price_class}, {context_band}, {USAGE_PRICING_AGGREGATE_COLUMNS},
         0 AS totals_start, {USAGE_TOTAL_COLUMNS}
         FROM ({projection}) GROUP BY 1, 2, 3, 4, 5",
            price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
            context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL,
        ))
        .map_err(db_error)?;
    // Do not duplicate the shared pricing decoder's column count here.
    let totals_offset = statement.column_index("totals_start").map_err(db_error)? + 1;
    let mut rows = statement
        .query(params_from_iter(parameters.iter().chain(parameters.iter())))
        .map_err(db_error)?;
    let mut sends = Projection::default();
    let mut sources = Vec::new();
    while let Some(row) = rows.next().map_err(db_error)? {
        let kind: String = row.get(0).map_err(db_error)?;
        let member: String = row.get(1).map_err(db_error)?;
        let model: String = row.get(2).map_err(db_error)?;
        let model_ref = (!model.is_empty()).then_some(model.as_str());
        let mut totals = usage_totals_from_row(row, totals_offset).map_err(db_error)?;
        totals.api_equivalent = resolver.estimate(
            &kind,
            &member,
            model_ref,
            usage_pricing_usage_from_row(row, 5).map_err(db_error)?,
        );
        sources.push(resolver.source(&kind, &member, model_ref));
        sends.add(query, &model, &member, totals);
    }
    Ok(ReportSummary {
        sends: sends.totals.requests,
        totals: priced_totals(requests.totals, sends.totals),
        models: groups(requests.models, sends.models),
        members: groups(requests.members, sends.members),
        sources,
    })
}

impl TelemetryDb {
    /// The all-time view includes archived pricing rollups and reuses
    /// incrementally maintained totals.
    pub(in crate::local_pool::store::telemetry_db) fn all_time_report_summary(
        &self,
        connection: &ReportRead<'_>,
        query: &UsageQuery,
        where_sql: &str,
        parameters: &[SqlValue],
        resolver: &CatalogPriceResolver<'_>,
    ) -> Result<ReportSummary> {
        let mut totals = self.cached_usage_totals(connection, query, where_sql, parameters)?;
        let mut models = if query.includes_models() {
            usage_groups(
                connection,
                where_sql,
                parameters,
                "COALESCE(resolved_model, requested_model, '')",
            )?
        } else {
            Vec::new()
        };
        let equivalents =
            usage_model_equivalents(connection, where_sql, parameters, resolver, true)?;
        let mut by_model = equivalents.models;
        for group in &mut models {
            group.totals.api_equivalent = by_model.remove(&group.key).unwrap_or_default();
            totals.api_equivalent.merge(group.totals.api_equivalent);
        }
        // Include every estimate when groups are omitted, and unmatched rollups
        // when retained history is no longer present in the request log.
        for estimate in by_model.values() {
            totals.api_equivalent.merge(*estimate);
        }
        let mut members = if query.includes_pool_members() {
            usage_groups(
                connection,
                where_sql,
                parameters,
                "COALESCE(account_id, source_id, '')",
            )?
        } else {
            Vec::new()
        };
        for group in &mut members {
            group.totals.api_equivalent = equivalents
                .members
                .get(&group.key)
                .copied()
                .unwrap_or_default();
        }
        let sends = send_projection("1", where_sql);
        let sends = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM ({sends})"),
                params_from_iter(parameters.iter().chain(parameters.iter())),
                |row| row.get::<_, i64>(0).map(|count| count.max(0) as u64),
            )
            .map_err(db_error)?;
        Ok(ReportSummary {
            totals,
            models,
            members,
            sources: equivalents.sources,
            sends,
        })
    }
}

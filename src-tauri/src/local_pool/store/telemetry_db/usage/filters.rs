use super::{send_projection, usage_filter};
use crate::local_pool::{
    error::Result,
    store::telemetry_db::{db_error, UsageFilterOptions},
};
use rusqlite::{params_from_iter, Connection};
use std::collections::BTreeSet;
use zenith_relay_core::protocol::UsageQuery;

pub(in crate::local_pool::store::telemetry_db) fn usage_filter_options(
    connection: &Connection,
    query: &UsageQuery,
) -> Result<UsageFilterOptions> {
    // Keep alternatives available after selecting a filter or changing pages.
    // Only the normalized period bounds constrain the available values.
    let (where_sql, parameters) = usage_filter(&UsageQuery {
        from_ms: query.from_ms,
        to_ms: query.to_ms,
        ..UsageQuery::default()
    });
    let history = send_projection(
        "wire_api, transport, error_category, requested_model, resolved_model, account_id, source_id",
        &where_sql,
    );
    let mut statement = connection
        .prepare(&format!(
            "SELECT DISTINCT CASE WHEN wire_api = 'chatcompletions' \
             THEN 'chat_completions' ELSE wire_api END, transport, error_category, \
             requested_model, resolved_model, COALESCE(account_id, source_id, '') \
             FROM ({history})"
        ))
        .map_err(db_error)?;
    let mut rows = statement
        .query(params_from_iter(parameters.iter().chain(parameters.iter())))
        .map_err(db_error)?;
    let mut values: [BTreeSet<String>; 6] = std::array::from_fn(|_| BTreeSet::new());
    while let Some(row) = rows.next().map_err(db_error)? {
        for (column, options) in values.iter_mut().enumerate() {
            if let Some(value) = row
                .get::<_, Option<String>>(column)
                .map_err(db_error)?
                .filter(|value| !value.is_empty())
            {
                options.insert(value);
            }
        }
    }
    let [wire_apis, transports, error_categories, requested_models, resolved_models, pool_members] =
        values;
    Ok(UsageFilterOptions {
        models: requested_models.union(&resolved_models).cloned().collect(),
        pool_members: pool_members.into_iter().collect(),
        wire_apis: wire_apis.into_iter().collect(),
        transports: transports.into_iter().collect(),
        error_categories: error_categories.into_iter().collect(),
    })
}

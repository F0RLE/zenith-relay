use super::{send_projection, usage_filter};
use crate::local_pool::{
    error::Result,
    store::telemetry_db::{
        db_error, lock_error, CachedUsageFilterOptions, TelemetryDb, UsageFilterOptions,
    },
};
use rusqlite::{params_from_iter, Connection};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use zenith_relay_core::protocol::UsageQuery;

impl TelemetryDb {
    #[cfg(test)]
    pub(in crate::local_pool::store::telemetry_db) fn cached_usage_filter_options(
        &self,
        connection: &Connection,
        query: &UsageQuery,
    ) -> Result<UsageFilterOptions> {
        let revision = self.usage_revision.load(Ordering::Acquire);
        self.usage_filter_options_at_revision(connection, query, revision)
    }

    /// Read filter options at the pinned snapshot's revision.
    pub(in crate::local_pool::store::telemetry_db) fn usage_filter_options_at_revision(
        &self,
        connection: &Connection,
        query: &UsageQuery,
        revision: u64,
    ) -> Result<UsageFilterOptions> {
        {
            let cached = self.usage_filter_options_cache.lock().map_err(lock_error)?;
            if let Some(cached) = cached.as_ref().filter(|cached| {
                cached.usage_revision == revision
                    && cached.from_ms == query.from_ms
                    && cached.to_ms == query.to_ms
            }) {
                return Ok(cached.options.clone());
            }
        }
        #[cfg(test)]
        self.usage_filter_option_reads
            .fetch_add(1, Ordering::Relaxed);
        let options = usage_filter_options(connection, query)?;
        let mut cached = self.usage_filter_options_cache.lock().map_err(lock_error)?;
        if self.usage_revision.load(Ordering::Acquire) == revision {
            // Cache only the latest selected period.
            cached.replace(CachedUsageFilterOptions {
                usage_revision: revision,
                from_ms: query.from_ms,
                to_ms: query.to_ms,
                options: options.clone(),
            });
        }
        Ok(options)
    }
}

fn usage_filter_options(connection: &Connection, query: &UsageQuery) -> Result<UsageFilterOptions> {
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

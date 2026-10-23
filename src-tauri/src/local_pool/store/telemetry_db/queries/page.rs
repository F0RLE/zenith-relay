use super::super::*;
#[cfg(test)]
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

// Keep the projection shared by both readers in the order consumed by usage_log_from_row.
const USAGE_LOG_COLUMNS: &str =
    "id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
     local_key_id, source_id, candidate_id, account_id, requested_model,
     resolved_model, wire_api, transport, success, http_status, error_category, latency_ms,
     ttft_ms, generation_ms, input_tokens, cached_input_tokens,
     cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
     service_tier, applied_service_tier, routing_json, tool_use_json, error_origin,
     requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl,
     client_context_id, upstream_error_json";

impl TelemetryDb {
    #[cfg(test)]
    pub fn list(&self, limit: u16) -> Result<Vec<UsageLog>> {
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {USAGE_LOG_COLUMNS} FROM request_logs ORDER BY id DESC LIMIT ?1"
            ))
            .map_err(db_error)?;
        let logs = statement
            .query_map([limit.clamp(1, 500)], usage_log_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(logs)
    }

    #[cfg(test)]
    pub fn usage_page(&self, query: &UsageQuery) -> Result<LocalUsagePage> {
        self.usage_page_with_price_overrides(query, &BTreeMap::new(), &BTreeMap::new())
    }

    #[cfg(test)]
    pub fn usage_page_with_price_overrides(
        &self,
        query: &UsageQuery,
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &SourcePriceOverrides,
    ) -> Result<LocalUsagePage> {
        let catalog = test_pricing_catalog();
        let context = test_pricing_context(price_overrides, source_price_overrides);
        let resolver = CatalogPriceResolver::new(&catalog, &context);
        self.usage_page_with_resolver(query, &resolver)
    }

    pub fn usage_page_with_pricing(
        &self,
        query: &UsageQuery,
        catalog: &PricingCatalog,
        context: &PricingContext,
    ) -> Result<LocalUsagePage> {
        let resolver = CatalogPriceResolver::new(catalog, context);
        self.usage_page_with_resolver(query, &resolver)
    }

    /// Compute quota-window API equivalents without constructing a complete
    /// usage page for every account. The caller supplies exact account ids and
    /// already-normalized window bounds. Raw aggregates are read in one short
    /// database-lock scope, then pricing is resolved after the lock is released
    /// so request recording is not blocked by catalog lookups.
    pub fn account_api_equivalents_with_pricing(
        &self,
        windows: &[(String, u64, u64)],
        catalog: &PricingCatalog,
        context: &PricingContext,
    ) -> Result<std::collections::HashMap<String, ApiEquivalentSummary>> {
        if windows.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let resolver = CatalogPriceResolver::new(catalog, context);
        let usage_revision = self.usage_revision.load(Ordering::Acquire);
        let pricing_revision = resolver.revision_key().to_string();
        if let Some(cached) = self
            .quota_equivalent_cache
            .lock()
            .map_err(lock_error)?
            .as_ref()
            .filter(|cached| {
                cached.usage_revision == usage_revision
                    && cached.pricing_revision == pricing_revision
                    && cached.windows == windows
            })
        {
            return Ok(cached.equivalents_by_account.clone());
        }
        let aggregates = {
            let connection = self.lock_connection()?;
            windows
                .iter()
                .map(|(account_id, from_ms, to_ms)| {
                    Ok((
                        account_id.clone(),
                        account_pricing_aggregates(&connection, account_id, *from_ms, *to_ms)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?
        };
        let mut equivalents = std::collections::HashMap::with_capacity(windows.len());
        for (account_id, rows) in aggregates {
            let mut total = ApiEquivalentSummary::default();
            for (model, usage) in rows {
                total.merge(resolver.estimate(
                    "account",
                    &account_id,
                    (!model.is_empty()).then_some(model.as_str()),
                    usage,
                ));
            }
            equivalents.insert(account_id, total);
        }
        if self.usage_revision.load(Ordering::Acquire) == usage_revision {
            self.quota_equivalent_cache
                .lock()
                .map_err(lock_error)?
                .replace(CachedQuotaEquivalents {
                    usage_revision,
                    pricing_revision,
                    windows: windows.to_vec(),
                    equivalents_by_account: equivalents.clone(),
                });
        }
        Ok(equivalents)
    }

    fn usage_page_with_resolver(
        &self,
        query: &UsageQuery,
        resolver: &CatalogPriceResolver<'_>,
    ) -> Result<LocalUsagePage> {
        let (page, page_size) = query.normalized_page();
        let connection = self.lock_connection()?;
        let (where_sql, query_parameters) = usage_filter(query);
        let filter_options = query
            .include_filter_options
            .then(|| usage_filter_options(&connection, query))
            .transpose()?;
        let use_all_time_rollups = is_unfiltered_all_time(query);
        let mut totals =
            self.cached_usage_totals(&connection, query, &where_sql, &query_parameters)?;
        let mut model_groups = if query.includes_models() {
            usage_groups(
                &connection,
                &where_sql,
                &query_parameters,
                "COALESCE(resolved_model, requested_model, '')",
            )?
        } else {
            Vec::new()
        };
        let equivalents = usage_model_equivalents(
            &connection,
            &where_sql,
            &query_parameters,
            resolver,
            use_all_time_rollups,
        )?;
        let mut api_equivalents_by_model = equivalents.models;
        if query.includes_models() {
            for group in &mut model_groups {
                group.totals.api_equivalent = api_equivalents_by_model
                    .remove(&group.key)
                    .unwrap_or_default();
                totals.api_equivalent.merge(group.totals.api_equivalent);
            }
        }
        // Include every estimate when groups are omitted, and unmatched rollups
        // when retained history is no longer present in the request log.
        for estimate in api_equivalents_by_model.values() {
            totals.api_equivalent.merge(*estimate);
        }
        let mut pool_members = if query.includes_pool_members() {
            usage_groups(
                &connection,
                &where_sql,
                &query_parameters,
                "COALESCE(account_id, source_id, '')",
            )?
        } else {
            Vec::new()
        };
        for group in &mut pool_members {
            group.totals.api_equivalent = equivalents
                .members
                .get(&group.key)
                .copied()
                .unwrap_or_default();
        }
        let buckets = usage_buckets(&connection, &where_sql, &query_parameters, query, resolver)?;
        let send_count_sql = super::super::usage::send_projection("1", &where_sql);
        let total_event_count = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM ({send_count_sql})"),
                params_from_iter(query_parameters.iter().chain(query_parameters.iter())),
                |row| row.get::<_, i64>(0).map(|count| count.max(0) as u64),
            )
            .map_err(db_error)?;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let mut events = if query.includes_events() {
            usage_send_page(
                &connection,
                &where_sql,
                &query_parameters,
                page_size,
                offset,
            )?
        } else {
            Vec::new()
        };
        for event in &mut events {
            let candidate_kind = if event.account_id.is_some() {
                "account"
            } else {
                "source"
            };
            let candidate_id = event.account_id.as_deref().unwrap_or(&event.source_id);
            let model_id = event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref());
            (event.api_equivalent, event.api_equivalent_breakdown) = resolver.estimate_breakdown(
                candidate_kind,
                candidate_id,
                model_id,
                zenith_relay_core::ApiEquivalentUsage::from_reported_tokens(
                    event.input_tokens,
                    event.cached_input_tokens,
                    event.cache_write_input_tokens,
                    event.cache_write_ttl.as_deref(),
                    event.output_tokens,
                    event.total_tokens,
                )
                .with_observed_rates(event.applied_service_tier.as_deref(), event.input_tokens),
            );
        }
        let total_pages = UsageQuery::page_count(total_event_count, page_size);
        let pricing_metadata =
            resolver.pricing_metadata(totals.api_equivalent, &equivalents.sources);
        Ok(LocalUsagePage {
            events,
            filter_options,
            total: total_event_count,
            page,
            page_size,
            total_pages,
            totals,
            models: model_groups,
            pool_members,
            buckets,
            pricing: pricing_metadata,
        })
    }
}

/// Sort only storage keys, then decode the requested page under the same lock.
/// Loading full JSON rows before LIMIT makes first entry scale with all retained
/// diagnostic payloads, even though the screen renders only 50 sends.
fn usage_send_page(
    connection: &rusqlite::Connection,
    where_sql: &str,
    parameters: &[SqlValue],
    page_size: u32,
    offset: u64,
) -> Result<Vec<UsageLog>> {
    let conjunction = if where_sql.is_empty() {
        " WHERE "
    } else {
        " AND "
    };
    let legacy = super::super::usage::LEGACY_SEND_PREDICATE;
    let sql = format!(
        "SELECT rowid AS storage_id, 1 AS is_attempt, created_at, ABS(id) AS sort_id
         FROM request_attempts{where_sql}
         UNION ALL
         SELECT rowid AS storage_id, 0 AS is_attempt, created_at, ABS(id) AS sort_id
         FROM request_logs{where_sql}{conjunction}{legacy}
         ORDER BY created_at DESC, sort_id DESC LIMIT ? OFFSET ?"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let mut page_parameters: Vec<_> = parameters
        .iter()
        .chain(parameters.iter())
        .cloned()
        .collect();
    page_parameters.push(SqlValue::Integer(i64::from(page_size)));
    page_parameters.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(offset)));
    let keys = statement
        .query_map(params_from_iter(page_parameters.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(db_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let mut attempts = connection
        .prepare(&format!(
            "SELECT {USAGE_LOG_COLUMNS} FROM request_attempts WHERE rowid = ?1"
        ))
        .map_err(db_error)?;
    let mut requests = connection
        .prepare(&format!(
            "SELECT {USAGE_LOG_COLUMNS} FROM request_logs WHERE rowid = ?1"
        ))
        .map_err(db_error)?;
    keys.into_iter()
        .map(|(rowid, is_attempt)| {
            let statement = if is_attempt {
                &mut attempts
            } else {
                &mut requests
            };
            statement
                .query_row([rowid], usage_log_from_row)
                .map_err(db_error)
        })
        .collect()
}

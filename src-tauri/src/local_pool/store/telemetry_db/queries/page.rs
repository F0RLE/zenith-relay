use super::super::*;
#[cfg(test)]
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

impl TelemetryDb {
    #[cfg(test)]
    pub fn list(&self, limit: u16) -> Result<Vec<UsageLog>> {
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                    local_key_id, source_id, candidate_id, account_id, requested_model,
                    resolved_model, wire_api, transport, success, http_status, error_category, latency_ms,
                    ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json, error_origin,
                    requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl,
                    client_context_id, upstream_error_json
                 FROM request_logs ORDER BY id DESC LIMIT ?1",
            )
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
        let use_all_time_rollups = is_unfiltered_all_time(query);
        let mut totals =
            self.cached_usage_totals(&connection, query, &where_sql, &query_parameters)?;
        let mut models = if query.includes_models() {
            usage_groups(
                &connection,
                &where_sql,
                &query_parameters,
                "COALESCE(resolved_model, requested_model, '')",
            )?
        } else {
            Vec::new()
        };
        let (mut model_equivalents, pricing_sources) = usage_model_equivalents(
            &connection,
            &where_sql,
            &query_parameters,
            resolver,
            use_all_time_rollups,
        )?;
        if query.includes_models() {
            for group in &mut models {
                group.totals.api_equivalent =
                    model_equivalents.remove(&group.key).unwrap_or_default();
                totals.api_equivalent.merge(group.totals.api_equivalent);
            }
            // Rollups also contain retained history that is no longer present
            // in the short-lived request log. Include those model totals in
            // the all-time projection instead of dropping the unmatched keys.
            for estimate in model_equivalents.values() {
                totals.api_equivalent.merge(*estimate);
            }
        } else {
            for estimate in model_equivalents.values() {
                totals.api_equivalent.merge(*estimate);
            }
        }
        let pool_members = if query.includes_pool_members() {
            usage_groups(
                &connection,
                &where_sql,
                &query_parameters,
                "COALESCE(account_id, source_id, '')",
            )?
        } else {
            Vec::new()
        };
        let buckets = usage_buckets(&connection, &where_sql, &query_parameters, query, resolver)?;
        let total = totals.requests;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let mut events = if query.includes_events() {
            let sql = format!(
                "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                    local_key_id, source_id, candidate_id, account_id, requested_model,
                    resolved_model, wire_api, transport, success, http_status, error_category, latency_ms,
                    ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json, error_origin,
                    requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl,
                    client_context_id, upstream_error_json
                 FROM request_logs{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
            );
            let mut statement = connection.prepare(&sql).map_err(db_error)?;
            let mut event_parameters = query_parameters;
            event_parameters.push(SqlValue::Integer(i64::from(page_size)));
            event_parameters.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(offset)));
            let events = statement
                .query_map(
                    params_from_iter(event_parameters.iter()),
                    usage_log_from_row,
                )
                .map_err(db_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_error)?;
            events
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
            event.api_equivalent = resolver.estimate(
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
        let total_pages = UsageQuery::page_count(total, page_size);
        let pricing_metadata = resolver.pricing_metadata(totals.api_equivalent, &pricing_sources);
        Ok(LocalUsagePage {
            events,
            total,
            page,
            page_size,
            total_pages,
            totals,
            models,
            pool_members,
            buckets,
            pricing: pricing_metadata,
        })
    }
}

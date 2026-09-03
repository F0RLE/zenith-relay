use super::*;
#[cfg(test)]
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

impl TelemetryDb {
    #[cfg(test)]
    pub fn list(&self, limit: u16) -> Result<Vec<UsageLog>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                    local_key_id, source_id, candidate_id, account_id, requested_model,
                    resolved_model, wire_api, success, http_status, error_category, latency_ms,
                    ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json, error_origin,
                    requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl,
                    client_context_id
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
            return Ok(cached.value.clone());
        }
        let aggregates = {
            let connection = self
                .connection
                .lock()
                .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
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
                    value: equivalents.clone(),
                });
        }
        Ok(equivalents)
    }

    fn usage_page_with_resolver(
        &self,
        query: &UsageQuery,
        resolver: &dyn UsagePriceResolver,
    ) -> Result<LocalUsagePage> {
        let page = query.page.max(1);
        let page_size = if query.page_size == 0 {
            50
        } else {
            query.page_size.clamp(1, 200)
        };
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let (where_sql, values) = usage_filter(query);
        let use_all_time_rollups = is_unfiltered_all_time(query);
        let mut totals = self.cached_usage_totals(&connection, query, &where_sql, &values)?;
        let mut models = if query.includes_models() {
            usage_groups(
                &connection,
                &where_sql,
                &values,
                "COALESCE(resolved_model, requested_model, '')",
            )?
        } else {
            Vec::new()
        };
        let (mut model_equivalents, pricing_sources) = usage_model_equivalents(
            &connection,
            &where_sql,
            &values,
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
                &values,
                "COALESCE(account_id, source_id, '')",
            )?
        } else {
            Vec::new()
        };
        let buckets = usage_buckets(&connection, &where_sql, &values, query, resolver)?;
        let total = totals.requests;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let mut events = if query.includes_events() {
            let sql = format!(
                "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                    local_key_id, source_id, candidate_id, account_id, requested_model,
                    resolved_model, wire_api, success, http_status, error_category, latency_ms,
                    ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json, error_origin,
                    requested_reasoning_effort, effective_reasoning_effort, cache_write_ttl,
                    client_context_id
                 FROM request_logs{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
            );
            let mut statement = connection.prepare(&sql).map_err(db_error)?;
            let mut page_values = values;
            page_values.push(SqlValue::Integer(i64::from(page_size)));
            page_values.push(SqlValue::Integer(offset.min(i64::MAX as u64) as i64));
            let events = statement
                .query_map(params_from_iter(page_values.iter()), usage_log_from_row)
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
            let model = event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref());
            let (cache_write_5m, cache_write_1h, unknown_cache_write) = match event.cache_write_ttl
            {
                Some(zenith_relay_core::CacheWriteTtl::FiveMinutes) => {
                    (event.cache_write_input_tokens, Some(0), Some(0))
                }
                Some(zenith_relay_core::CacheWriteTtl::OneHour) => {
                    (Some(0), event.cache_write_input_tokens, Some(0))
                }
                _ => (Some(0), Some(0), event.cache_write_input_tokens),
            };
            event.api_equivalent = resolver.estimate(
                candidate_kind,
                candidate_id,
                model,
                zenith_relay_core::ApiEquivalentUsage {
                    input_tokens: event.input_tokens,
                    cached_input_tokens: event.cached_input_tokens,
                    cache_write_5m_tokens: cache_write_5m,
                    cache_write_1h_tokens: cache_write_1h,
                    unknown_cache_write_tokens: unknown_cache_write,
                    output_tokens: event.output_tokens,
                    total_tokens: event.total_tokens,
                },
            );
        }
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(u64::from(page_size)) as u32
        };
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

    #[cfg(test)]
    pub fn api_equivalents(&self) -> Result<UsageEquivalents> {
        self.api_equivalents_with_price_overrides(&BTreeMap::new(), &BTreeMap::new())
    }

    #[cfg(test)]
    pub fn api_equivalents_with_price_overrides(
        &self,
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &SourcePriceOverrides,
    ) -> Result<UsageEquivalents> {
        let catalog = test_pricing_catalog();
        let context = test_pricing_context(price_overrides, source_price_overrides);
        let resolver = CatalogPriceResolver::new(&catalog, &context);
        self.api_equivalents_with_resolver(&resolver)
    }

    pub fn api_equivalents_with_pricing(
        &self,
        catalog: &PricingCatalog,
        context: &PricingContext,
    ) -> Result<UsageEquivalents> {
        let resolver = CatalogPriceResolver::new(catalog, context);
        self.api_equivalents_with_resolver(&resolver)
    }

    fn api_equivalents_with_resolver(
        &self,
        resolver: &dyn UsagePriceResolver,
    ) -> Result<UsageEquivalents> {
        let usage_revision = self.usage_revision.load(Ordering::Acquire);
        let pricing_revision = resolver.revision_key().to_string();
        if let Some(cached) = self
            .api_equivalent_cache
            .lock()
            .map_err(lock_error)?
            .as_ref()
            .filter(|cached| {
                cached.usage_revision == usage_revision
                    && cached.pricing_revision == pricing_revision
            })
        {
            return Ok(cached.value.clone());
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT candidate_kind, candidate_id, model,
                    input_tokens, cached_input_tokens, cache_write_input_tokens,
                    cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
                    output_tokens, total_tokens, input_samples,
                    cached_input_samples, cache_write_input_samples
                 FROM usage_candidate_rollups",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let input_tokens: Option<i64> = row.get(3)?;
                let cached_input_tokens: Option<i64> = row.get(4)?;
                let cache_write_5m_tokens: Option<i64> = row.get(6)?;
                let cache_write_1h_tokens: Option<i64> = row.get(7)?;
                let unknown_cache_write_tokens: Option<i64> = row.get(8)?;
                let output_tokens: Option<i64> = row.get(9)?;
                let total_tokens: Option<i64> = row.get(10)?;
                let input_samples: i64 = row.get(11)?;
                let cached_samples: i64 = row.get(12)?;
                let cache_write_samples: i64 = row.get(13)?;
                let model = row.get::<_, Option<String>>(2)?;
                let kind = row.get::<_, String>(0)?;
                let id = row.get::<_, String>(1)?;
                Ok((
                    kind.clone(),
                    id.clone(),
                    resolver.estimate(
                        &kind,
                        &id,
                        model.as_deref(),
                        zenith_relay_core::ApiEquivalentUsage {
                            input_tokens: input_tokens.map(rust_u64),
                            cached_input_tokens: (input_samples > 0
                                && cached_samples == input_samples)
                                .then(|| cached_input_tokens.map(rust_u64))
                                .flatten(),
                            cache_write_5m_tokens: (cache_write_samples > 0)
                                .then(|| cache_write_5m_tokens.map(rust_u64).unwrap_or_default()),
                            cache_write_1h_tokens: (cache_write_samples > 0)
                                .then(|| cache_write_1h_tokens.map(rust_u64).unwrap_or_default()),
                            unknown_cache_write_tokens: (cache_write_samples > 0).then(|| {
                                unknown_cache_write_tokens.map(rust_u64).unwrap_or_default()
                            }),
                            output_tokens: output_tokens.map(rust_u64),
                            total_tokens: total_tokens.map(rust_u64),
                        },
                    ),
                ))
            })
            .map_err(db_error)?;
        let mut equivalents = UsageEquivalents::default();
        for row in rows {
            let (kind, id, estimate) = row.map_err(db_error)?;
            let values = if kind == "account" {
                &mut equivalents.accounts
            } else {
                &mut equivalents.sources
            };
            values.entry(id).or_default().merge(estimate);
        }
        drop(statement);
        drop(connection);
        if self.usage_revision.load(Ordering::Acquire) == usage_revision {
            self.api_equivalent_cache
                .lock()
                .map_err(lock_error)?
                .replace(CachedUsageEquivalents {
                    usage_revision,
                    pricing_revision,
                    value: equivalents.clone(),
                });
        }
        Ok(equivalents)
    }
}

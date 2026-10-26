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
     client_context_id, upstream_error_json, image_usage_json";

impl TelemetryDb {
    pub(crate) fn request_diagnostics(&self, request_id: &str) -> Result<serde_json::Value> {
        use rusqlite::OptionalExtension;
        let connection = self.lock_connection()?;
        let final_request = connection
            .query_row(
                &format!("SELECT {USAGE_LOG_COLUMNS} FROM request_logs WHERE request_id = ?1"),
                [request_id],
                usage_log_from_row,
            )
            .optional()
            .map_err(db_error)?;
        let mut statement = connection.prepare(&format!(
            "SELECT {USAGE_LOG_COLUMNS} FROM request_attempts WHERE request_id = ?1 ORDER BY attempt DESC LIMIT 65"
        )).map_err(db_error)?;
        let mut attempts = statement
            .query_map([request_id], usage_log_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let reasoning_samples =
            super::super::support::reasoning::read_samples(&connection, final_request.as_ref())?;
        // Rendering, sorting and hashing the bounded report must not hold up the writer.
        drop(statement);
        drop(connection);
        if final_request.is_none() && attempts.is_empty() {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "No retained usage for this request",
            ));
        }
        let truncated = attempts.len() > 64;
        attempts.truncate(64);
        attempts.reverse();
        Ok(super::super::support::document_with_comparison(
            final_request.as_ref(),
            &attempts,
            truncated,
            Some(super::super::support::reasoning::document(
                reasoning_samples,
            )),
        ))
    }

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
    /// already-normalized window bounds. Raw aggregates share one read snapshot;
    /// pricing runs after releasing it. Request recording keeps its writer.
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
        // The result has one value per account. Preserve the previous last-
        // window-wins behavior for duplicate IDs without repeated SQL reads.
        let requested_windows = windows
            .iter()
            .map(|(id, from, to)| (id.clone(), (*from, *to)))
            .collect::<std::collections::HashMap<_, _>>();
        let mut equivalents = std::collections::HashMap::with_capacity(requested_windows.len());
        let mut missing = Vec::new();
        {
            let cached = self.quota_equivalent_cache.lock().map_err(lock_error)?;
            let cached = cached
                .as_ref()
                .filter(|cached| cached.pricing_revision == pricing_revision);
            for (id, bounds) in &requested_windows {
                let value = cached
                    .filter(|cached| cached.windows.get(id) == Some(bounds))
                    .and_then(|cached| cached.equivalents_by_account.get(id));
                if let Some(value) = value {
                    equivalents.insert(id.clone(), *value);
                } else {
                    missing.push((id, bounds));
                }
            }
        }
        if missing.is_empty() {
            return Ok(equivalents);
        }
        let aggregates = {
            let connection = self.lock_report_connection()?;
            missing
                .iter()
                .map(|(account_id, (from_ms, to_ms))| {
                    #[cfg(test)]
                    self.quota_window_reads.fetch_add(1, Ordering::Relaxed);
                    Ok((
                        (*account_id).clone(),
                        account_pricing_aggregates(&connection, account_id, *from_ms, *to_ms)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?
        };
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
        // Check after taking the cache lock: a writer/clear that invalidated
        // an earlier read must not be followed by publication of stale sums.
        self.publish_quota_equivalents(
            usage_revision,
            CachedQuotaEquivalents {
                pricing_revision,
                windows: requested_windows,
                equivalents_by_account: equivalents.clone(),
            },
        )?;
        Ok(equivalents)
    }

    pub(in crate::local_pool::store::telemetry_db) fn publish_quota_equivalents(
        &self,
        expected_revision: u64,
        values: CachedQuotaEquivalents,
    ) -> Result<bool> {
        let mut cached = self.quota_equivalent_cache.lock().map_err(lock_error)?;
        if self.usage_revision.load(Ordering::Acquire) != expected_revision {
            return Ok(false);
        }
        cached.replace(values);
        Ok(true)
    }

    fn usage_page_with_resolver(
        &self,
        query: &UsageQuery,
        resolver: &CatalogPriceResolver<'_>,
    ) -> Result<LocalUsagePage> {
        let (page, page_size) = query.normalized_page();
        let connection = self.lock_report_connection()?;
        let usage_revision = connection.revision;
        let (where_sql, query_parameters) = usage_filter(query);
        let filter_options = query
            .include_filter_options
            .then(|| self.usage_filter_options_at_revision(&connection, query, usage_revision))
            .transpose()?;
        // Pagination does not change period totals. Reuse one bounded summary
        // for the same exact scope, history and pricing revision.
        let mut summary_query = query.clone();
        summary_query.page = 1;
        summary_query.page_size = 1;
        summary_query.include_events = Some(false);
        summary_query.include_filter_options = false;
        let query_key = serde_json::to_string(&summary_query).map_err(|_| {
            LocalPoolError::new(ErrorCode::Io, "Usage scope could not be serialized")
        })?;
        let pricing_revision = resolver.revision_key().to_string();
        let cached = self
            .usage_report_cache
            .lock()
            .map_err(lock_error)?
            .as_ref()
            .filter(|cached| {
                cached.query_key == query_key
                    && cached.usage_revision == usage_revision
                    && cached.pricing_revision == pricing_revision
            })
            .map(|cached| cached.page.clone());
        if let Some(mut cached) = cached {
            let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
            cached.events = if query.includes_events() {
                usage_send_page(
                    &connection,
                    &where_sql,
                    &query_parameters,
                    page_size,
                    offset,
                    None,
                )?
            } else {
                Vec::new()
            };
            cached.filter_options = filter_options;
            cached.page = page;
            cached.page_size = page_size;
            cached.total_pages = UsageQuery::page_count(cached.total, page_size);
            drop(connection);
            price_events(&mut cached.events, resolver);
            // Catalog health/timestamps can change without a price revision.
            // Preserve the aggregate's price-source classification, not stale
            // freshness metadata from its first page.
            cached.pricing = PricingMetadata {
                price_source: cached.pricing.price_source,
                ..resolver.pricing_metadata(cached.totals.api_equivalent, &[])
            };
            return Ok(cached);
        }
        #[cfg(test)]
        self.usage_report_builds.fetch_add(1, Ordering::Relaxed);
        let summary = if is_unfiltered_all_time(query) {
            self.all_time_report_summary(
                &connection,
                query,
                &where_sql,
                &query_parameters,
                resolver,
            )?
        } else {
            super::super::usage::scoped_report_summary(
                &connection,
                query,
                &where_sql,
                &query_parameters,
                resolver,
            )?
        };
        let buckets = usage_buckets(&connection, &where_sql, &query_parameters, query, resolver)?;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let mut events = if query.includes_events() {
            usage_send_page(
                &connection,
                &where_sql,
                &query_parameters,
                page_size,
                offset,
                None,
            )?
        } else {
            Vec::new()
        };
        // All report rows belong to the same read snapshot. Pricing the owned
        // page below no longer needs to block request recording.
        drop(connection);
        price_events(&mut events, resolver);
        let total_pages = UsageQuery::page_count(summary.sends, page_size);
        let pricing_metadata =
            resolver.pricing_metadata(summary.totals.api_equivalent, &summary.sources);
        let mut result = LocalUsagePage {
            events,
            has_more: None,
            workload: query.workload,
            filter_options,
            total: summary.sends,
            page,
            page_size,
            total_pages,
            totals: summary.totals,
            models: summary.models,
            pool_members: summary.members,
            buckets,
            pricing: pricing_metadata,
        };
        // Do not clone the selected page's potentially large diagnostic DTOs.
        let events = std::mem::take(&mut result.events);
        let filters = result.filter_options.take();
        let mut cache = self.usage_report_cache.lock().map_err(lock_error)?;
        if self.usage_revision.load(Ordering::Acquire) == usage_revision {
            *cache =
                CachedUsageReport::fits_cache(&query_key, &pricing_revision, &result).then(|| {
                    CachedUsageReport {
                        query_key,
                        usage_revision,
                        pricing_revision,
                        page: result.clone(),
                    }
                });
        }
        result.events = events;
        result.filter_options = filters;
        Ok(result)
    }
}

pub(super) fn price_events(events: &mut [UsageLog], resolver: &CatalogPriceResolver<'_>) {
    for event in events {
        let kind = if event.account_id.is_some() {
            "account"
        } else {
            "source"
        };
        let member = event.account_id.as_deref().unwrap_or(&event.source_id);
        let model = event
            .resolved_model
            .as_deref()
            .or(event.requested_model.as_deref());
        (event.api_equivalent, event.api_equivalent_breakdown) = resolver.estimate_breakdown(
            kind,
            member,
            model,
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
}

/// Sort only storage keys, then decode the requested page under the same lock.
/// Loading full JSON rows before LIMIT makes first entry scale with all retained
/// diagnostic payloads, even though the screen renders only 50 sends.
pub(super) fn usage_send_page(
    connection: &rusqlite::Connection,
    where_sql: &str,
    parameters: &[SqlValue],
    page_size: u32,
    offset: u64,
    has_more: Option<&mut bool>,
) -> Result<Vec<UsageLog>> {
    use rusqlite::OptionalExtension;

    let limit = page_size + u32::from(has_more.is_some());
    let mut cutoff_parameters = parameters.to_vec();
    cutoff_parameters.push(SqlValue::Integer(sql_u64(
        offset.saturating_add(u64::from(limit.saturating_sub(1))),
    )));
    let cutoff = connection
        .prepare_cached(&format!(
            "SELECT created_at, ABS(id) FROM request_attempts{where_sql}
             ORDER BY created_at DESC, ABS(id) DESC LIMIT 1 OFFSET ?"
        ))
        .map_err(db_error)?
        .query_row(params_from_iter(cutoff_parameters.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .optional()
        .map_err(db_error)?;
    // A complete attempt prefix bounds every legacy send that can enter this
    // page. Keep ties; older keys already rank after the whole selected prefix.
    let legacy_bound = if cutoff.is_some() {
        " AND (created_at, ABS(id)) >= (?, ?)"
    } else {
        ""
    };
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
         FROM request_logs{where_sql}{conjunction}{legacy}{legacy_bound}
         ORDER BY created_at DESC, sort_id DESC LIMIT ? OFFSET ?"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let mut page_parameters: Vec<_> = parameters
        .iter()
        .chain(parameters.iter())
        .cloned()
        .collect();
    if let Some((created_at, sort_id)) = cutoff {
        page_parameters.push(SqlValue::Text(created_at));
        page_parameters.push(SqlValue::Integer(sort_id));
    }
    page_parameters.push(SqlValue::Integer(i64::from(limit)));
    page_parameters.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(offset)));
    let mut keys = statement
        .query_map(params_from_iter(page_parameters.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(db_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)?;
    if let Some(has_more) = has_more {
        *has_more = keys.len() > page_size as usize;
        // Do not decode even one diagnostic row from the next page.
        keys.truncate(page_size as usize);
    }
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

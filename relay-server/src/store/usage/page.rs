use super::super::sqlite::{db_error, optional_u64, Store};
#[cfg(test)]
use super::pricing::{test_pricing_catalog, test_pricing_context};
use super::query::{
    usage_buckets, usage_filter, usage_groups, usage_model_equivalents, usage_totals,
};
use rusqlite::{params_from_iter, types::Value as SqlValue};
use zenith_relay_core::CatalogPriceResolver;
use zenith_relay_core::{
    normalize_observed_service_tier,
    pricing::{PricingCatalog, PricingContext},
    protocol::{UsagePage, UsageQuery, UsageSummary, UsageTokenBreakdown},
    ApiEquivalentSummary, ApiEquivalentUsage, DefaultServiceTier, WireApi,
};

impl Store {
    #[cfg(test)]
    pub fn usage_page(&self, query: &UsageQuery) -> Result<UsagePage, String> {
        let price_overrides = self.model_price_overrides()?;
        let source_price_overrides = self.source_price_overrides()?;
        let catalog = test_pricing_catalog();
        let context = test_pricing_context(&price_overrides, &source_price_overrides);
        let resolver = CatalogPriceResolver::new(&catalog, &context);
        self.usage_page_with_resolver(query, &resolver)
    }

    pub fn usage_page_with_pricing(
        &self,
        query: &UsageQuery,
        catalog: &PricingCatalog,
        context: &PricingContext,
    ) -> Result<UsagePage, String> {
        let resolver = CatalogPriceResolver::new(catalog, context);
        self.usage_page_with_resolver(query, &resolver)
    }

    fn usage_page_with_resolver(
        &self,
        query: &UsageQuery,
        resolver: &CatalogPriceResolver<'_>,
    ) -> Result<UsagePage, String> {
        let (page, page_size) = query.normalized_page();
        let connection = self.lock()?;
        let (where_sql, query_parameters) = usage_filter(query);
        let mut totals = usage_totals(&connection, &where_sql, &query_parameters)?;
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
        let (mut model_equivalents, pricing_sources) =
            usage_model_equivalents(&connection, &where_sql, &query_parameters, resolver)?;
        if query.includes_models() {
            for group in &mut models {
                group.totals.api_equivalent =
                    model_equivalents.remove(&group.key).unwrap_or_default();
                totals.api_equivalent.merge(group.totals.api_equivalent);
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
                "COALESCE(candidate_hint, '')",
            )?
        } else {
            Vec::new()
        };
        let buckets = usage_buckets(&connection, &where_sql, &query_parameters, query, resolver)?;
        let total = totals.requests;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let mut events = if query.includes_events() {
            let sql = format!(
                "SELECT id, request_id, local_key_id, candidate_kind, candidate_hint, \
                 requested_model, resolved_model, wire_api, transport, success, http_status, error_category, \
                 latency_ms, ttft_ms, generation_ms, input_tokens, cached_input_tokens, \
                 cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens, \
                 created_at_ms, routing_json, service_tier, applied_service_tier, tool_use_json, \
                 error_origin, requested_reasoning_effort, effective_reasoning_effort, \
                 cache_write_ttl, attempt, upstream_error_json \
                 FROM usage_events{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
            );
            let mut statement = connection.prepare(&sql).map_err(db_error)?;
            let mut event_parameters = query_parameters;
            event_parameters.push(SqlValue::Integer(i64::from(page_size)));
            event_parameters.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(offset)));
            let rows = statement
                .query_map(params_from_iter(event_parameters.iter()), map_usage_event)
                .map_err(db_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?
        } else {
            Vec::new()
        };
        for event in &mut events {
            let model_id = event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref());
            event.api_equivalent = resolver.estimate(
                &event.candidate_kind,
                &event.candidate_hint,
                model_id,
                ApiEquivalentUsage::from_reported_tokens(
                    event.tokens.input_tokens,
                    event.tokens.cached_input_tokens,
                    event.tokens.cache_write_input_tokens,
                    event.tokens.cache_write_ttl.as_deref(),
                    event.tokens.output_tokens,
                    event.tokens.total_tokens,
                )
                .with_observed_rates(
                    event.applied_service_tier.as_deref(),
                    event.tokens.input_tokens,
                ),
            );
        }
        let total_pages = UsageQuery::page_count(total, page_size);
        let pricing_metadata = resolver.pricing_metadata(totals.api_equivalent, &pricing_sources);
        Ok(UsagePage {
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

fn map_usage_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageSummary> {
    let wire_api: String = row.get(7)?;
    Ok(UsageSummary {
        id: row.get(0)?,
        request_id: row.get(1)?,
        attempt: row.get::<_, i64>(30)?.clamp(0, i64::from(u16::MAX)) as u16,
        candidate_kind: row.get(3)?,
        candidate_hint: row.get(4)?,
        candidate_label: None,
        routing: row
            .get::<_, Option<String>>(22)?
            .as_deref()
            .and_then(|routing_json| serde_json::from_str(routing_json).ok()),
        requested_model: row.get(5)?,
        resolved_model: row.get(6)?,
        requested_reasoning_effort: row
            .get::<_, Option<String>>(27)?
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        effective_reasoning_effort: row
            .get::<_, Option<String>>(28)?
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        wire_api: WireApi::from_storage_value(&wire_api).unwrap_or(WireApi::Responses),
        transport: row
            .get::<_, Option<String>>(8)?
            .as_deref()
            .and_then(|transport_text| transport_text.parse().ok())
            .unwrap_or_default(),
        service_tier: DefaultServiceTier::from_storage_value(&row.get::<_, String>(23)?),
        applied_service_tier: row
            .get::<_, Option<String>>(24)?
            .as_deref()
            .and_then(normalize_observed_service_tier),
        tool_use: row
            .get::<_, Option<String>>(25)?
            .as_deref()
            .and_then(|tool_use_json| serde_json::from_str(tool_use_json).ok()),
        success: row.get::<_, i64>(9)? != 0,
        http_status: row.get::<_, i64>(10)?.clamp(0, i64::from(u16::MAX)) as u16,
        error_category: row.get(11)?,
        upstream_error: row
            .get::<_, Option<String>>(31)?
            .as_deref()
            .and_then(|upstream_error_json| serde_json::from_str(upstream_error_json).ok()),
        error_origin: row
            .get::<_, Option<String>>(26)?
            .as_deref()
            .and_then(|error_origin_text| error_origin_text.parse().ok()),
        latency_ms: row.get::<_, i64>(12)?.max(0) as u64,
        ttft_ms: optional_u64(row.get(13)?),
        generation_ms: optional_u64(row.get(14)?),
        tokens: UsageTokenBreakdown {
            input_tokens: optional_u64(row.get(15)?),
            cached_input_tokens: optional_u64(row.get(16)?),
            cache_write_input_tokens: optional_u64(row.get(17)?),
            cache_write_ttl: row
                .get::<_, Option<String>>(29)?
                .as_deref()
                .and_then(zenith_relay_core::usage::normalize_reported_cache_ttls),
            reasoning_tokens: optional_u64(row.get(18)?),
            output_tokens: optional_u64(row.get(19)?),
            total_tokens: optional_u64(row.get(20)?),
        },
        api_equivalent: ApiEquivalentSummary::default(),
        created_at_ms: row.get::<_, i64>(21)?.max(0) as u64,
    })
}

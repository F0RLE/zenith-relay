use super::super::sqlite::{db_error, Store};
#[cfg(test)]
use super::pricing::{test_pricing_catalog, test_pricing_context};
use super::query;
use std::collections::HashMap;
use zenith_relay_core::usage::CANDIDATE_ROLLUP_TOKEN_OFFSET;
use zenith_relay_core::CatalogPriceResolver;
use zenith_relay_core::{
    pricing::{PricingCatalog, PricingContext},
    ApiEquivalentSummary, ApiEquivalentUsage, ObservedUsageSums,
};

impl Store {
    #[cfg(test)]
    pub fn api_equivalents(&self) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
        let price_overrides = self.model_price_overrides()?;
        let source_price_overrides = self.source_price_overrides()?;
        let catalog = test_pricing_catalog();
        let context = test_pricing_context(&price_overrides, &source_price_overrides);
        let resolver = CatalogPriceResolver::new(&catalog, &context);
        self.api_equivalents_with_resolver(&resolver)
    }

    pub fn api_equivalents_with_pricing(
        &self,
        catalog: &PricingCatalog,
        context: &PricingContext,
    ) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
        let resolver = CatalogPriceResolver::new(catalog, context);
        self.api_equivalents_with_resolver(&resolver)
    }

    /// Price only the raw account rows inside each quota window.
    ///
    /// Archived rollups have no window bounds, and a full usage page also
    /// builds events, groups, and buckets that this projection does not use.
    pub fn quota_window_equivalents_with_pricing(
        &self,
        windows: &[(String, u64, u64)],
        catalog: &PricingCatalog,
        context: &PricingContext,
    ) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
        if windows.is_empty() {
            return Ok(HashMap::new());
        }
        let resolver = CatalogPriceResolver::new(catalog, context);
        let connection = self.lock()?;
        let mut equivalents = HashMap::with_capacity(windows.len());
        for (hint, from_ms, to_ms) in windows {
            let mut total = ApiEquivalentSummary::default();
            for (model, usage) in
                query::candidate_window_usage(&connection, hint, *from_ms, *to_ms)?
            {
                total.merge(resolver.estimate(
                    "account",
                    hint,
                    (!model.is_empty()).then_some(model.as_str()),
                    usage,
                ));
            }
            equivalents.insert(hint.clone(), total);
        }
        Ok(equivalents)
    }

    fn api_equivalents_with_resolver(
        &self,
        resolver: &CatalogPriceResolver<'_>,
    ) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT candidate_kind, candidate_id, model, price_class, context_band,
                    SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
                    SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(unknown_cache_write_tokens),
                    SUM(output_tokens), SUM(total_tokens), SUM(input_samples),
                    SUM(cached_input_samples), SUM(cache_write_input_samples)
                 FROM (
                    SELECT candidate_kind, candidate_id, model, price_class, context_band,
                        input_tokens, cached_input_tokens, cache_write_input_tokens,
                        0 AS cache_write_5m_tokens, 0 AS cache_write_1h_tokens,
                        cache_write_input_tokens AS unknown_cache_write_tokens,
                        output_tokens, total_tokens, input_samples,
                        cached_input_samples, cache_write_input_samples
                    FROM usage_candidate_rollups
                    UNION ALL
                    SELECT candidate_kind, candidate_hint,
                        COALESCE(resolved_model, requested_model, ''),
                        {price_class}, {context_band},
                        COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
                        COALESCE(SUM(cache_write_input_tokens), 0),
                        {cache_write_buckets},
                        COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(total_tokens), 0), COUNT(input_tokens),
                        COUNT(cached_input_tokens), COUNT(cache_write_input_tokens)
                    FROM usage_events GROUP BY 1, 2, 3, 4, 5
                 ) GROUP BY candidate_kind, candidate_id, model, price_class, context_band",
                cache_write_buckets = zenith_relay_core::usage::CACHE_WRITE_TTL_BUCKET_SUMS_SQL,
                price_class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
                context_band = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL
            ))
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let kind = row.get::<_, String>(0)?;
                let candidate_id = row.get::<_, String>(1)?;
                let model = row.get::<_, Option<String>>(2)?;
                let price_class: String = row.get(3)?;
                let context_band: String = row.get(4)?;
                Ok((candidate_id.clone(), {
                    resolver.estimate(
                        &kind,
                        &candidate_id,
                        model.as_deref(),
                        ApiEquivalentUsage::from_observed_sums(
                            ObservedUsageSums::from_rollup_aggregate(
                                |column| row.get(CANDIDATE_ROLLUP_TOKEN_OFFSET + column),
                                |column| row.get(CANDIDATE_ROLLUP_TOKEN_OFFSET + column),
                            )?,
                        )
                        .with_aggregate_rates(&price_class, &context_band),
                    )
                }))
            })
            .map_err(db_error)?;
        let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
        for row in rows {
            let (candidate_hint, estimate) = row.map_err(db_error)?;
            equivalents
                .entry(candidate_hint)
                .or_default()
                .merge(estimate);
        }
        Ok(equivalents)
    }
}

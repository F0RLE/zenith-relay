use super::super::*;
#[cfg(test)]
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use zenith_relay_core::usage::CANDIDATE_ROLLUP_TOKEN_OFFSET;

impl TelemetryDb {
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
        resolver: &CatalogPriceResolver<'_>,
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
        let connection = self.lock_connection()?;
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
                        zenith_relay_core::ApiEquivalentUsage::from_observed_sums(
                            zenith_relay_core::ObservedUsageSums::from_rollup_aggregate(
                                |column| row.get(CANDIDATE_ROLLUP_TOKEN_OFFSET + column),
                                |column| row.get(CANDIDATE_ROLLUP_TOKEN_OFFSET + column),
                            )?,
                        ),
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

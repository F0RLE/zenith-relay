use super::{CatalogEntry, PriceSource, PricingCatalog, ResolvedPrice, TokenPrice};

impl PricingCatalog {
    /// Resolves a source price using provider evidence, then LiteLLM exact,
    /// official-family canonical matching, and finally a manual override.
    pub fn resolve_source(
        &self,
        model: &str,
        pricing_provider: Option<&str>,
        official_provider_family: Option<&str>,
        provider_price: Option<TokenPrice>,
        manual_price: Option<TokenPrice>,
    ) -> ResolvedPrice {
        if let Some(price) = provider_price {
            return self.resolved(price, PriceSource::Provider);
        }
        if let Some(entry) = self.exact_entry(model, pricing_provider) {
            if let Some(price) = entry.token {
                return self.resolved(price, PriceSource::LiteLlmExact);
            }
        }
        if let Some(family) = official_provider_family {
            if let Some(entry) = self.canonical_entry(model, family) {
                if let Some(price) = entry.token {
                    return self.resolved(price, PriceSource::LiteLlmCanonical);
                }
            }
        }
        manual_price.map_or_else(
            || ResolvedPrice::unpriced(self.metadata()),
            |price| self.resolved(price, PriceSource::Manual),
        )
    }

    /// Account pricing is intentionally isolated to the official family.
    pub fn resolve_account(&self, model: &str, provider_family: Option<&str>) -> ResolvedPrice {
        provider_family
            .and_then(|family| self.canonical_entry(model, family))
            .and_then(|entry| entry.token)
            .map_or_else(
                || ResolvedPrice::unpriced(self.metadata()),
                |price| self.resolved(price, PriceSource::LiteLlmCanonical),
            )
    }

    fn exact_entry(&self, model: &str, provider: Option<&str>) -> Option<&CatalogEntry> {
        let model = normalize(model);
        if let Some(provider) = provider.map(normalize) {
            // Callers may pass either the public bare id or a provider-qualified
            // id. Build the qualified lookup from the unqualified component so
            // both forms address the same LiteLLM record.
            let qualified = format!("{provider}/{}", unqualified(&model));
            if let Some(entry) = self.unique.get(&qualified) {
                if entry.provider.as_deref() == Some(provider.as_str()) {
                    return Some(entry);
                }
            }
            if let Some(entry) = self.unique.get(&model) {
                if entry.provider.as_deref() == Some(provider.as_str()) {
                    return Some(entry);
                }
            }
            if let Some(entry) = self.unique.get(&unqualified(&model)) {
                if entry.provider.as_deref() == Some(provider.as_str()) {
                    return Some(entry);
                }
            }
            // A provider was explicitly requested. Do not silently select a
            // similarly named record belonging to another provider.
            return None;
        }
        self.unique.get(&model)
    }

    fn canonical_entry(&self, model: &str, family: &str) -> Option<&CatalogEntry> {
        // Canonical matching intentionally ignores the input namespace. The
        // declared family below is the authority for which namespace is safe.
        let model = unqualified(model);
        let family = normalize(family);
        let candidates = self
            .entries
            .values()
            .filter(|entry| {
                entry.provider.as_deref() == Some(family.as_str())
                    && unqualified(&entry.model_id) == model
                    && !self.conflicts.contains(&normalize(&entry.model_id))
            })
            .collect::<Vec<_>>();
        let first = candidates.first().copied()?;
        // Qualified and unqualified LiteLLM aliases are equivalent when they
        // carry the same quote. Conflicting aliases remain unusable.
        candidates
            .iter()
            .all(|candidate| candidate.equivalent_pricing(first))
            .then_some(first)
    }

    fn resolved(&self, quote: TokenPrice, source: PriceSource) -> ResolvedPrice {
        ResolvedPrice {
            quote: Some(quote),
            source,
            catalog_revision: self.revision.clone(),
            catalog_fetched_at_ms: self.fetched_at_ms,
            stale: self.stale,
        }
    }

    pub(crate) fn metadata(&self) -> (Option<String>, Option<u64>, bool) {
        (self.revision.clone(), self.fetched_at_ms, self.stale)
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn unqualified(value: &str) -> String {
    value
        .rsplit_once('/')
        .map_or_else(|| normalize(value), |(_, model)| normalize(model))
}

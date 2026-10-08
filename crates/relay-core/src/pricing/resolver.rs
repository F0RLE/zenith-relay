use super::{CatalogEntry, PriceSource, PricingCatalog, ResolvedPrice, TokenPrice};

impl PricingCatalog {
    /// Resolves a source price using provider evidence, then LiteLLM exact,
    /// the official catalog family for a known model id, an explicitly
    /// declared family for any other model, and finally a manual override.
    /// The known family comes only from the model id.
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
        if let Some(pricing_entry) = self.exact_entry(model, pricing_provider) {
            if let Some(price) = pricing_entry.token {
                return self.resolved(price, PriceSource::LiteLlmExact);
            }
        }
        // A recognized model keeps its own official family even when the
        // source declared a different one. Unrecognized models can still use
        // that explicit declaration. Neither choice reads a source label or URL.
        let family = Self::official_model_family(model).or(official_provider_family);
        if let Some(family) = family {
            if let Some(pricing_entry) = self.canonical_entry(model, family) {
                if let Some(price) = pricing_entry.token {
                    return self.resolved(price, PriceSource::LiteLlmCanonical);
                }
            }
        }
        manual_price.map_or_else(
            || ResolvedPrice::unpriced(self.metadata()),
            |price| self.resolved(price, PriceSource::Manual),
        )
    }

    /// Official LiteLLM family for a model id Relay already knows.
    ///
    /// Prefixes cover later versions without a version list. Qualified ids use
    /// the unqualified model component, so `xai/grok-4.7` and `grok-4.7` share
    /// one family. Gemini's official catalog namespace is `gemini`.
    fn official_model_family(model: &str) -> Option<&'static str> {
        let model_id = super::unqualified(model);
        if model_id.starts_with("gpt-")
            || model_id.starts_with("chatgpt-")
            || model_id.starts_with("codex-")
        {
            Some("openai")
        } else if model_id.starts_with("claude-") {
            Some("anthropic")
        } else if model_id.starts_with("gemini-") {
            Some("gemini")
        } else if model_id.starts_with("grok-") {
            Some("xai")
        } else {
            None
        }
    }

    /// Account pricing is intentionally isolated to the official family.
    pub fn resolve_account(&self, model: &str, provider_family: Option<&str>) -> ResolvedPrice {
        provider_family
            .and_then(|family| self.canonical_entry(model, family))
            .and_then(|pricing_entry| pricing_entry.token)
            .map_or_else(
                || ResolvedPrice::unpriced(self.metadata()),
                |price| self.resolved(price, PriceSource::LiteLlmCanonical),
            )
    }

    fn exact_entry(&self, model: &str, provider: Option<&str>) -> Option<&CatalogEntry> {
        let normalized_model_id = super::normalize(model);
        if let Some(provider) = provider.map(super::normalize) {
            // Callers may pass either the public bare id or a provider-qualified
            // id. Build the qualified lookup from the unqualified component so
            // both forms address the same LiteLLM record.
            let qualified = format!("{provider}/{}", super::unqualified(&normalized_model_id));
            if let Some(pricing_entry) = self.unique.get(&qualified) {
                if pricing_entry.provider.as_deref() == Some(provider.as_str()) {
                    return Some(pricing_entry);
                }
            }
            if let Some(pricing_entry) = self.unique.get(&normalized_model_id) {
                if pricing_entry.provider.as_deref() == Some(provider.as_str()) {
                    return Some(pricing_entry);
                }
            }
            if let Some(pricing_entry) = self.unique.get(&super::unqualified(&normalized_model_id))
            {
                if pricing_entry.provider.as_deref() == Some(provider.as_str()) {
                    return Some(pricing_entry);
                }
            }
            // A provider was explicitly requested. Do not silently select a
            // similarly named record belonging to another provider.
            return None;
        }
        self.unique.get(&normalized_model_id)
    }

    fn canonical_entry(&self, model: &str, family: &str) -> Option<&CatalogEntry> {
        // Canonical matching intentionally ignores the input namespace. The
        // declared family below is the authority for which namespace is safe.
        let unqualified_model_id = super::unqualified(model);
        let normalized_family = super::normalize(family);
        let candidates = self
            .entries
            .values()
            .filter(|candidate_entry| {
                candidate_entry.provider.as_deref() == Some(normalized_family.as_str())
                    && super::unqualified(&candidate_entry.model_id) == unqualified_model_id
                    && !self
                        .conflicts
                        .contains(&super::normalize(&candidate_entry.model_id))
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

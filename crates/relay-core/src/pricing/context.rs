use super::{PriceEvidence, PricingCatalog, ResolvedPrice, SourcePricingMetadata, TokenPrice};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeMap;

/// Immutable, storage-neutral context used by usage and snapshot builders.
/// Candidate keys are the redacted ids used by the host storage layer, so the
/// context never needs secrets or raw account identities.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingContext {
    pub account_provider_families: BTreeMap<String, String>,
    pub source_metadata: BTreeMap<String, SourcePricingMetadata>,
    pub source_evidence: BTreeMap<String, BTreeMap<String, PriceEvidence>>,
    /// Legacy/global manual source overrides. They remain source-only and are
    /// never consulted for account candidates.
    #[serde(default)]
    pub global_manual_prices: BTreeMap<String, TokenPrice>,
}

impl PricingContext {
    pub fn candidate_price(
        &self,
        catalog: &PricingCatalog,
        candidate_kind: &str,
        candidate_id: &str,
        model: Option<&str>,
    ) -> ResolvedPrice {
        let Some(model) = crate::omit_blank(model) else {
            return ResolvedPrice::unpriced(catalog.metadata());
        };
        let context_id = if candidate_kind.eq_ignore_ascii_case("source") {
            source_context_id(candidate_id)
        } else {
            candidate_id.to_string()
        };
        let evidence = self
            .source_evidence
            .get(&context_id)
            .or_else(|| self.source_evidence.get(&super::normalize(&context_id)))
            .and_then(|prices| prices.get(&super::normalize(model)).copied());
        if candidate_kind.eq_ignore_ascii_case("account") {
            return catalog.resolve_account(
                model,
                self.account_provider_families
                    .get(candidate_id)
                    .or_else(|| {
                        self.account_provider_families
                            .get(&super::normalize(candidate_id))
                    })
                    .map(String::as_str)
                    .or(Some("openai")),
            );
        }
        let metadata = self
            .source_metadata
            .get(&context_id)
            .or_else(|| self.source_metadata.get(&super::normalize(&context_id)));
        let mut resolved = catalog.resolve_source(
            model,
            metadata.and_then(|source_metadata| source_metadata.pricing_provider.as_deref()),
            metadata
                .and_then(|source_metadata| source_metadata.official_provider_family.as_deref()),
            evidence.and_then(|price_evidence| price_evidence.provider),
            evidence
                .and_then(|price_evidence| price_evidence.manual)
                .or_else(|| {
                    self.global_manual_prices
                        .get(&super::normalize(model))
                        .copied()
                }),
        );
        let cache_write_allowed = metadata.is_some_and(|source_metadata| {
            source_metadata
                .cache_write_models
                .contains(&super::normalize(model))
        });
        if !cache_write_allowed {
            if let Some(quote) = resolved.quote.as_mut() {
                *quote = quote.clear_cache_writes();
            }
        }
        resolved
    }

    /// A deterministic key for host-side derived caches.  The catalog
    /// revision is included first; changing any explicit source/account
    /// evidence also invalidates the result without touching usage rows.
    pub fn revision_key(&self, catalog: &PricingCatalog) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        let mut bytes = catalog.revision.clone().unwrap_or_default().into_bytes();
        bytes.push(0);
        bytes.extend_from_slice(&encoded);
        let digest = sha2::Sha256::digest(bytes);
        format!(
            "{}:{}",
            catalog.revision.as_deref().unwrap_or("unloaded"),
            hex::encode(digest)
        )
    }
}

fn source_context_id(candidate_id: &str) -> String {
    let Some((source_id, suffix)) = candidate_id.rsplit_once("::") else {
        return candidate_id.to_string();
    };
    let known_route = suffix == "bridge"
        || crate::WireApi::ALL.iter().any(|client| {
            crate::WireApi::ALL.iter().any(|upstream| {
                crate::SourceAdapter::between(*client, *upstream)
                    .is_some_and(|adapter| adapter.route_suffix(*client) == suffix)
            })
        });
    if known_route {
        source_id.to_string()
    } else {
        candidate_id.to_string()
    }
}

use super::{CatalogStatus, PricingCatalog, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenPrice {
    pub input: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_5m: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
    pub output: u64,
}

impl TokenPrice {
    pub const fn is_valid(self) -> bool {
        self.input <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
            && self.output <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
            && option_is_valid(self.cache_read)
            && option_is_valid(self.cache_write_5m)
            && option_is_valid(self.cache_write_1h)
    }
}

const fn option_is_valid(value: Option<u64>) -> bool {
    match value {
        Some(value) => value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
        None => true,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageModelPrice {
    pub input_micro_usd_per_image: Option<u64>,
    pub output_micro_usd_per_image: Option<u64>,
    pub input_micro_usd_per_image_token: Option<u64>,
}

/// A request-level image quote. Image operations are intentionally kept out
/// of [`TokenPrice`]: a price per generated image is not a price per token.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequestPrice {
    pub operation: String,
    pub quality: String,
    pub size: String,
    pub micro_usd: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PriceSource {
    Provider,
    LiteLlmExact,
    LiteLlmCanonical,
    Manual,
    #[default]
    Unpriced,
}

impl PriceSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::LiteLlmExact => "liteLlmExact",
            Self::LiteLlmCanonical => "liteLlmCanonical",
            Self::Manual => "manual",
            Self::Unpriced => "unpriced",
        }
    }
}

/// Provenance for an aggregate. A page can contain rows resolved from more
/// than one source, so `Mixed` is explicit instead of selecting an arbitrary
/// row's provenance.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PricingSourceSummary {
    Provider,
    LiteLlmExact,
    LiteLlmCanonical,
    Manual,
    Mixed,
    #[default]
    Unpriced,
}

impl PricingSourceSummary {
    pub fn from_sources<I>(sources: I) -> Self
    where
        I: IntoIterator<Item = PriceSource>,
    {
        let mut result = None;
        for source in sources {
            if source == PriceSource::Unpriced {
                continue;
            }
            result = Some(match result {
                None => source,
                Some(previous) if previous == source => previous,
                Some(_) => return Self::Mixed,
            });
        }
        match result {
            Some(PriceSource::Provider) => Self::Provider,
            Some(PriceSource::LiteLlmExact) => Self::LiteLlmExact,
            Some(PriceSource::LiteLlmCanonical) => Self::LiteLlmCanonical,
            Some(PriceSource::Manual) => Self::Manual,
            Some(PriceSource::Unpriced) | None => Self::Unpriced,
        }
    }
}

/// Catalog metadata attached to usage and runtime projections. It describes
/// the quote source and freshness, not a provider debit or customer charge.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_fetched_at_ms: Option<u64>,
    #[serde(default)]
    pub catalog_stale: bool,
    /// Loader state is separate from `catalog_stale`: an old immutable
    /// snapshot can remain usable while a refresh is in flight or has failed.
    #[serde(default)]
    pub catalog_status: CatalogStatus,
    #[serde(default)]
    pub price_source: PricingSourceSummary,
    #[serde(default)]
    pub unpriced_tokens: u64,
}

impl PricingMetadata {
    pub fn for_catalog(
        catalog: &PricingCatalog,
        source: PricingSourceSummary,
        unpriced_tokens: u64,
    ) -> Self {
        let status = if catalog.stale {
            CatalogStatus::Stale
        } else if catalog.revision.is_some() {
            CatalogStatus::Current
        } else {
            CatalogStatus::Unloaded
        };
        Self::for_catalog_with_status(catalog, status, source, unpriced_tokens)
    }

    pub fn for_catalog_with_status(
        catalog: &PricingCatalog,
        status: CatalogStatus,
        source: PricingSourceSummary,
        unpriced_tokens: u64,
    ) -> Self {
        Self {
            catalog_revision: catalog.revision.clone(),
            catalog_fetched_at_ms: catalog.fetched_at_ms,
            catalog_stale: catalog.stale,
            catalog_status: status,
            price_source: source,
            unpriced_tokens,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPrice {
    pub quote: Option<TokenPrice>,
    pub source: PriceSource,
    pub catalog_revision: Option<String>,
    pub catalog_fetched_at_ms: Option<u64>,
    pub stale: bool,
}

impl ResolvedPrice {
    pub(super) fn unpriced(metadata: (Option<String>, Option<u64>, bool)) -> Self {
        Self {
            quote: None,
            source: PriceSource::Unpriced,
            catalog_revision: metadata.0,
            catalog_fetched_at_ms: metadata.1,
            stale: metadata.2,
        }
    }
}

/// Provider/manual evidence attached to one source and model.  The two
/// values are deliberately kept separate so a stale provider record cannot
/// silently replace an operator override (or vice versa).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceEvidence {
    pub provider: Option<TokenPrice>,
    pub manual: Option<TokenPrice>,
}

/// Explicit pricing identity for a compatible API source.  `pricing_provider`
/// is the LiteLLM provider namespace (for example `openrouter`), while
/// `official_provider_family` is an opt-in canonical fallback (for example
/// `openai`).  Neither value is inferred from a display name or URL.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePricingMetadata {
    pub pricing_provider: Option<String>,
    pub official_provider_family: Option<String>,
    /// Models for which this source has a confirmed Anthropic-style Messages
    /// route. Cache creation prices are stripped for every other source/model
    /// combination, including manual and LiteLLM fallback prices.
    #[serde(default)]
    pub cache_write_models: BTreeSet<String>,
}

use super::{CatalogStatus, PricingCatalog, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRateSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_5m: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
}

impl TokenRateSet {
    pub const EMPTY: Self = Self {
        input: None,
        cache_read: None,
        cache_write_5m: None,
        cache_write_1h: None,
        output: None,
    };

    pub const fn is_empty(self) -> bool {
        self.input.is_none()
            && self.cache_read.is_none()
            && self.cache_write_5m.is_none()
            && self.cache_write_1h.is_none()
            && self.output.is_none()
    }

    fn skip_serializing(rate_set: &Self) -> bool {
        rate_set.is_empty()
    }

    pub const fn is_valid(self) -> bool {
        option_is_valid(self.input)
            && option_is_valid(self.cache_read)
            && option_is_valid(self.cache_write_5m)
            && option_is_valid(self.cache_write_1h)
            && option_is_valid(self.output)
    }

    pub const fn overlay(self, above: Self) -> Self {
        Self {
            input: or_rate(above.input, self.input),
            cache_read: or_rate(above.cache_read, self.cache_read),
            cache_write_5m: or_rate(above.cache_write_5m, self.cache_write_5m),
            cache_write_1h: or_rate(above.cache_write_1h, self.cache_write_1h),
            output: or_rate(above.output, self.output),
        }
    }

    const fn clear_cache_writes(self) -> Self {
        Self {
            cache_write_5m: None,
            cache_write_1h: None,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LongContextRates {
    #[serde(default, skip_serializing_if = "TokenRateSet::skip_serializing")]
    pub standard: TokenRateSet,
    #[serde(default, skip_serializing_if = "TokenRateSet::skip_serializing")]
    pub flex: TokenRateSet,
    #[serde(default, skip_serializing_if = "TokenRateSet::skip_serializing")]
    pub priority: TokenRateSet,
}

impl LongContextRates {
    pub const EMPTY: Self = Self {
        standard: TokenRateSet::EMPTY,
        flex: TokenRateSet::EMPTY,
        priority: TokenRateSet::EMPTY,
    };

    pub const fn is_empty(self) -> bool {
        self.standard.is_empty() && self.flex.is_empty() && self.priority.is_empty()
    }

    fn skip_serializing(context_rates: &Self) -> bool {
        context_rates.is_empty()
    }

    pub const fn is_valid(self) -> bool {
        self.standard.is_valid() && self.flex.is_valid() && self.priority.is_valid()
    }

    const fn clear_cache_writes(self) -> Self {
        Self {
            standard: self.standard.clear_cache_writes(),
            flex: self.flex.clear_cache_writes(),
            priority: self.priority.clear_cache_writes(),
        }
    }
}

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
    /// Published Flex components. An empty set means this catalog has no Flex
    /// price; callers must not reuse the standard components for that tier.
    #[serde(default, skip_serializing_if = "TokenRateSet::skip_serializing")]
    pub flex: TokenRateSet,
    /// Published Priority components. `fast` uses this set. Ultrafast does not.
    #[serde(default, skip_serializing_if = "TokenRateSet::skip_serializing")]
    pub priority: TokenRateSet,
    #[serde(default, skip_serializing_if = "LongContextRates::skip_serializing")]
    pub above_200k: LongContextRates,
    #[serde(default, skip_serializing_if = "LongContextRates::skip_serializing")]
    pub above_272k: LongContextRates,
}

impl TokenPrice {
    pub const fn is_valid(self) -> bool {
        self.input <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
            && self.output <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
            && option_is_valid(self.cache_read)
            && option_is_valid(self.cache_write_5m)
            && option_is_valid(self.cache_write_1h)
            && self.flex.is_valid()
            && self.priority.is_valid()
            && self.above_200k.is_valid()
            && self.above_272k.is_valid()
    }

    pub const fn clear_cache_writes(self) -> Self {
        Self {
            cache_write_5m: None,
            cache_write_1h: None,
            flex: self.flex.clear_cache_writes(),
            priority: self.priority.clear_cache_writes(),
            above_200k: self.above_200k.clear_cache_writes(),
            above_272k: self.above_272k.clear_cache_writes(),
            ..self
        }
    }
}

const fn option_is_valid(rate_value: Option<u64>) -> bool {
    match rate_value {
        Some(rate_value) => rate_value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
        None => true,
    }
}

const fn or_rate(preferred_rate: Option<u64>, fallback_rate: Option<u64>) -> Option<u64> {
    match preferred_rate {
        Some(rate_value) => Some(rate_value),
        None => fallback_rate,
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
        let mut resolved_source = None;
        for source in sources {
            if source == PriceSource::Unpriced {
                continue;
            }
            resolved_source = Some(match resolved_source {
                None => source,
                Some(previous_source) if previous_source == source => previous_source,
                Some(_) => return Self::Mixed,
            });
        }
        match resolved_source {
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
        let catalog_status = if catalog.stale {
            CatalogStatus::Stale
        } else if catalog.revision.is_some() {
            CatalogStatus::Current
        } else {
            CatalogStatus::Unloaded
        };
        Self::for_catalog_with_status(catalog, catalog_status, source, unpriced_tokens)
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

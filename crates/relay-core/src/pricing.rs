mod catalog;
mod decimal;
mod litellm_parser;
mod loader;
mod resolver;
mod schedule;

pub(crate) use decimal::decimal_to_scaled_allow_zero;

pub use catalog::{
    payload_hash, PricingCacheEnvelope, MAX_CACHE_BYTES, MAX_CACHE_RECORDS, MAX_CACHE_STRING_LENGTH,
};
pub use decimal::{
    usd_per_request_to_micro_usd, usd_per_token_to_micro_usd_per_million, usd_to_micro,
};
pub use loader::{
    CatalogRefreshOutcome, CatalogStatus, PricingCacheStore, PricingCatalogLoader,
    DEFAULT_CATALOG_MAX_AGE_MS, MAX_CATALOG_RESPONSE_BYTES,
};
pub use schedule::{
    pricing_refresh_delay, pricing_refresh_jitter_seconds, CatalogRefreshDeadline,
    CatalogRefreshKind, PRICING_REFRESH_INTERVAL_SECONDS, PRICING_REFRESH_JITTER_MAX_SECONDS,
};

pub const LITELLM_SOURCE_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
pub const CACHE_FORMAT: &str = "zenith-relay-litellm-cache";
pub const CACHE_SCHEMA_VERSION: u32 = 1;
pub const MAX_MODEL_PRICE_MICRO_USD_PER_MILLION: u64 = 1_000_000_000_000;

mod context;
mod quote;
mod snapshot;

pub use context::PricingContext;
pub use quote::{
    ImageModelPrice, ImageRequestPrice, LongContextRates, PriceEvidence, PriceSource,
    PricingMetadata, PricingSourceSummary, ResolvedPrice, SourcePricingMetadata, TokenPrice,
    TokenRateSet,
};
pub use snapshot::{CatalogEntry, PricingCatalog, PricingCatalogHandle, PricingError};

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Rejected operator pricing provider or official family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPricingIdentity;

/// Stores a pricing provider or official family in canonical form.
///
/// Absent input stays absent. A present value is trimmed and lowercased.
/// It may contain only ASCII letters, digits, `.`, `-`, and `_`, and must
/// be at most 128 characters. Whitespace-only input is invalid, not absent.
pub fn normalize_pricing_identity(
    value: Option<String>,
) -> Result<Option<String>, InvalidPricingIdentity> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = normalize(&value);
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(InvalidPricingIdentity);
    }
    Ok(Some(value))
}

fn unqualified(value: &str) -> String {
    value
        .rsplit_once('/')
        .map_or_else(|| normalize(value), |(_, model)| normalize(model))
}

#[cfg(test)]
mod tests;

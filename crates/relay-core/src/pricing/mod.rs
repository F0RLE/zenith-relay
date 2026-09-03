mod catalog;
mod decimal;
mod litellm_parser;
mod loader;
mod resolver;
mod schedule;

use catalog::validate_litellm_payload;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

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
    fn unpriced(metadata: (Option<String>, Option<u64>, bool)) -> Self {
        Self {
            quote: None,
            source: PriceSource::Unpriced,
            catalog_revision: metadata.0,
            catalog_fetched_at_ms: metadata.1,
            stale: metadata.2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub model_id: String,
    pub provider: Option<String>,
    pub token: Option<TokenPrice>,
    pub image: Option<ImageModelPrice>,
    pub request_micro_usd: Option<u64>,
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
}

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
        let Some(model) = model.filter(|value| !value.trim().is_empty()) else {
            return ResolvedPrice::unpriced(catalog.metadata());
        };
        let evidence = self
            .source_evidence
            .get(candidate_id)
            .or_else(|| self.source_evidence.get(&normalize(candidate_id)))
            .and_then(|prices| prices.get(&normalize(model)).copied());
        if candidate_kind.eq_ignore_ascii_case("account") {
            return catalog.resolve_account(
                model,
                self.account_provider_families
                    .get(candidate_id)
                    .or_else(|| self.account_provider_families.get(&normalize(candidate_id)))
                    .map(String::as_str)
                    .or(Some("openai")),
            );
        }
        let metadata = self
            .source_metadata
            .get(candidate_id)
            .or_else(|| self.source_metadata.get(&normalize(candidate_id)));
        catalog.resolve_source(
            model,
            metadata.and_then(|value| value.pricing_provider.as_deref()),
            metadata.and_then(|value| value.official_provider_family.as_deref()),
            evidence.and_then(|value| value.provider),
            evidence
                .and_then(|value| value.manual)
                .or_else(|| self.global_manual_prices.get(&normalize(model)).copied()),
        )
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

impl CatalogEntry {
    fn request_priced(model_id: String, provider: Option<String>, request_micro_usd: u64) -> Self {
        Self {
            model_id,
            provider,
            token: None,
            image: None,
            request_micro_usd: Some(request_micro_usd),
        }
    }

    /// Model ids are aliases in the LiteLLM document. When deciding whether
    /// two aliases can share an index entry, compare only their pricing
    /// identity; the spelling of the id itself is expected to differ.
    fn equivalent_pricing(&self, other: &Self) -> bool {
        self.provider == other.provider
            && self.token == other.token
            && self.image == other.image
            && self.request_micro_usd == other.request_micro_usd
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PricingCatalog {
    pub revision: Option<String>,
    pub fetched_at_ms: Option<u64>,
    pub stale: bool,
    pub entries: BTreeMap<String, CatalogEntry>,
    pub conflicts: BTreeSet<String>,
    unique: BTreeMap<String, CatalogEntry>,
}

impl PricingCatalog {
    pub fn empty() -> Self {
        Self {
            revision: None,
            fetched_at_ms: None,
            stale: false,
            entries: BTreeMap::new(),
            conflicts: BTreeSet::new(),
            unique: BTreeMap::new(),
        }
    }

    pub fn from_litellm_json(raw: &str) -> Result<Self, PricingError> {
        let payload = serde_json::from_str(raw).map_err(|_| PricingError::InvalidCatalog)?;
        Self::from_litellm_payload(&payload, None, None, false)
    }

    pub fn from_litellm_payload(
        payload: &Value,
        revision: Option<String>,
        fetched_at_ms: Option<u64>,
        stale: bool,
    ) -> Result<Self, PricingError> {
        let object = validate_litellm_payload(payload)?;
        let mut entries = BTreeMap::<String, CatalogEntry>::new();
        let mut unique = BTreeMap::<String, CatalogEntry>::new();
        let mut conflicts = BTreeSet::<String>::new();
        for (model_id, value) in object {
            if model_id.len() > MAX_CACHE_STRING_LENGTH {
                continue;
            }
            // LiteLLM is an external, evolving catalog. One malformed record
            // must not discard every valid model in the snapshot.
            let Some(entry) = (match litellm_parser::parse_entry(model_id, value) {
                Ok(entry) => entry,
                Err(_) => continue,
            }) else {
                continue;
            };
            let key = normalize(model_id);
            if let Some(existing) = unique.get(&key) {
                if !existing.equivalent_pricing(&entry) {
                    conflicts.insert(key.clone());
                    unique.remove(&key);
                }
            } else if !conflicts.contains(&key) {
                unique.insert(key.clone(), entry.clone());
            }
            entries.insert(model_id.clone(), entry);
        }
        Ok(Self {
            revision,
            fetched_at_ms,
            stale,
            entries,
            conflicts,
            unique,
        })
    }

    /// Returns a stable rank for a model in the current catalog. LiteLLM is a
    /// JSON object rather than an ordered model list, so the rank is only a
    /// deterministic presentation hint and must never affect routing.
    pub fn rank_for(&self, model: &str) -> Option<u32> {
        let normalized = normalize(model);
        self.entries
            .keys()
            .filter(|id| !self.conflicts.contains(&normalize(id)))
            .enumerate()
            .find_map(|(rank, id)| {
                (normalize(id) == normalized || unqualified(id) == normalized)
                    .then(|| u32::try_from(rank).ok())
                    .flatten()
            })
    }

    /// Projects LiteLLM image fields into request-level rows. LiteLLM often
    /// publishes one generic image tariff without quality/size dimensions;
    /// those dimensions are represented as `default` rather than guessed.
    pub fn image_request_prices(&self, model: &str) -> Vec<ImageRequestPrice> {
        let normalized = normalize(model);
        let Some(entry) = self
            .entries
            .values()
            .filter(|entry| {
                !self.conflicts.contains(&normalize(&entry.model_id))
                    && (normalize(&entry.model_id) == normalized
                        || unqualified(&entry.model_id) == normalized)
            })
            .find(|entry| entry.image.is_some())
        else {
            return Vec::new();
        };
        let Some(image) = entry.image else {
            return Vec::new();
        };
        let mut rows = Vec::with_capacity(2);
        if let Some(price) = image.output_micro_usd_per_image {
            rows.push(ImageRequestPrice {
                operation: "generation".to_string(),
                quality: "default".to_string(),
                size: "default".to_string(),
                micro_usd: price,
            });
        }
        if let Some(price) = image.input_micro_usd_per_image {
            rows.push(ImageRequestPrice {
                operation: "edit".to_string(),
                quality: "default".to_string(),
                size: "default".to_string(),
                micro_usd: price,
            });
        }
        rows
    }

    /// Returns a token quote for an explicitly declared official provider
    /// family. This helper keeps account/image callers from reaching into the
    /// resolver's matching internals.
    pub fn official_token_price(&self, model: &str, provider_family: &str) -> Option<TokenPrice> {
        self.resolve_account(model, Some(provider_family)).quote
    }

    /// Returns whether at least one non-conflicting token record matches the
    /// model. It is useful for capability checks where provenance is not
    /// needed.
    pub fn has_token_price(&self, model: &str) -> bool {
        self.entries.values().any(|entry| {
            entry.token.is_some()
                && !self.conflicts.contains(&normalize(&entry.model_id))
                && (normalize(&entry.model_id) == normalize(model)
                    || unqualified(&entry.model_id) == normalize(model))
        })
    }

    pub fn handle(self) -> PricingCatalogHandle {
        PricingCatalogHandle::new(self)
    }
}

#[derive(Clone, Debug)]
pub struct PricingCatalogHandle {
    current: Arc<RwLock<Arc<PricingCatalog>>>,
}

impl PricingCatalogHandle {
    pub fn new(catalog: PricingCatalog) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(catalog))),
        }
    }

    pub fn snapshot(&self) -> Arc<PricingCatalog> {
        self.current
            .read()
            .expect("pricing catalog lock poisoned")
            .clone()
    }

    pub fn replace(&self, catalog: PricingCatalog) {
        *self.current.write().expect("pricing catalog lock poisoned") = Arc::new(catalog);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PricingError {
    InvalidAmount,
    Overflow,
    InvalidRecord,
    InvalidCatalog,
    InvalidCache,
    CacheTooLarge,
    Io,
    Network,
    HttpStatus(u16),
}

impl std::fmt::Display for PricingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidAmount => "pricing amount is invalid",
            Self::Overflow => "pricing amount overflows supported range",
            Self::InvalidRecord => "LiteLLM pricing record is invalid",
            Self::InvalidCatalog => "LiteLLM pricing catalog is invalid",
            Self::InvalidCache => "pricing cache is invalid",
            Self::CacheTooLarge => "pricing cache exceeds safety limits",
            Self::Io => "pricing cache I/O failed",
            Self::Network => "pricing catalog refresh failed",
            Self::HttpStatus(status) => {
                return write!(formatter, "pricing catalog returned HTTP {status}")
            }
        })
    }
}

impl std::error::Error for PricingError {}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn unqualified(value: &str) -> String {
    value
        .rsplit_once('/')
        .map_or_else(|| normalize(value), |(_, model)| normalize(model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_provider_exact_then_canonical_then_manual() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "openrouter/gpt-test": {
                    "litellm_provider": "openrouter",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                },
                "gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 3e-6,
                    "output_cost_per_token": 4e-6
                }
            }),
            Some("sha256:test".into()),
            Some(10),
            true,
        )
        .unwrap();
        let manual = TokenPrice {
            input: 9,
            cache_read: None,
            cache_write_5m: None,
            cache_write_1h: None,
            output: 9,
        };
        let exact = catalog.resolve_source(
            "gpt-test",
            Some("openrouter"),
            Some("openai"),
            None,
            Some(manual),
        );
        assert_eq!(exact.source, PriceSource::LiteLlmExact);
        assert_eq!(exact.quote.unwrap().input, 1_000_000);
        let account = catalog.resolve_account("gpt-test", Some("openai"));
        assert_eq!(account.source, PriceSource::LiteLlmCanonical);
        assert_eq!(account.quote.unwrap().input, 3_000_000);
    }

    #[test]
    fn account_family_isolation_does_not_use_other_provider() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({"claude-sonnet": {
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-6,
                "output_cost_per_token": 15e-6
            }}),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            catalog
                .resolve_account("claude-sonnet", Some("openai"))
                .source,
            PriceSource::Unpriced
        );
    }

    #[test]
    fn conflicting_case_variants_are_not_silently_overwritten() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "GPT-Test": {"litellm_provider": "openai", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
                "gpt-test": {"litellm_provider": "openai", "input_cost_per_token": 3e-6, "output_cost_per_token": 4e-6}
            }),
            None,
            None,
            false,
        )
        .unwrap();
        assert!(catalog.conflicts.contains("gpt-test"));
        assert_eq!(
            catalog.resolve_account("gpt-test", Some("openai")).source,
            PriceSource::Unpriced
        );
    }

    #[test]
    fn equivalent_qualified_and_unqualified_aliases_resolve_for_accounts() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                },
                "openai/gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                }
            }),
            None,
            None,
            false,
        )
        .unwrap();

        assert!(catalog.conflicts.is_empty());
        let resolved = catalog.resolve_account("gpt-test", Some("openai"));
        assert_eq!(resolved.source, PriceSource::LiteLlmCanonical);
        assert_eq!(resolved.quote.unwrap().input, 1_000_000);
    }

    #[test]
    fn canonical_matching_accepts_a_qualified_model_id() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                }
            }),
            None,
            None,
            false,
        )
        .unwrap();

        let resolved = catalog.resolve_account("openai/gpt-test", Some("openai"));
        assert_eq!(resolved.source, PriceSource::LiteLlmCanonical);
        assert_eq!(resolved.quote.unwrap().input, 1_000_000);
    }

    #[test]
    fn conflicting_canonical_aliases_are_left_unpriced() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                },
                "openai/gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 3e-6,
                    "output_cost_per_token": 4e-6
                }
            }),
            None,
            None,
            false,
        )
        .unwrap();

        assert_eq!(
            catalog.resolve_account("gpt-test", Some("openai")).source,
            PriceSource::Unpriced
        );
    }

    #[test]
    fn explicit_provider_is_required_for_provider_specific_exact_prices() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "gpt-test": {
                    "litellm_provider": "openrouter",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                }
            }),
            None,
            None,
            false,
        )
        .unwrap();
        let manual = TokenPrice {
            input: 9,
            cache_read: None,
            cache_write_5m: None,
            cache_write_1h: None,
            output: 9,
        };

        let exact =
            catalog.resolve_source("openrouter/gpt-test", Some("openrouter"), None, None, None);
        assert_eq!(exact.source, PriceSource::LiteLlmExact);
        assert_eq!(exact.quote.unwrap().input, 1_000_000);

        let isolated = catalog.resolve_source("gpt-test", Some("openai"), None, None, Some(manual));
        assert_eq!(isolated.source, PriceSource::Manual);
        assert_eq!(isolated.quote.unwrap(), manual);
    }

    #[test]
    fn qualified_exact_match_cannot_cross_provider_namespaces() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "openrouter/gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                }
            }),
            None,
            None,
            false,
        )
        .unwrap();

        let resolved = catalog.resolve_source("gpt-test", Some("openrouter"), None, None, None);
        assert_eq!(resolved.source, PriceSource::Unpriced);
    }

    #[test]
    fn pricing_context_normalizes_source_ids_but_keeps_account_policy_isolated() {
        let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": 1e-6,
                    "output_cost_per_token": 2e-6
                }
            }),
            None,
            None,
            false,
        )
        .unwrap();
        let provider = TokenPrice {
            input: 7,
            cache_read: None,
            cache_write_5m: None,
            cache_write_1h: None,
            output: 7,
        };
        let context = PricingContext {
            account_provider_families: BTreeMap::from([("acct".into(), "openrouter".into())]),
            source_metadata: BTreeMap::from([(
                "source".into(),
                SourcePricingMetadata {
                    pricing_provider: Some("openrouter".into()),
                    official_provider_family: None,
                },
            )]),
            source_evidence: BTreeMap::from([(
                "source".into(),
                BTreeMap::from([(
                    "GPT-TEST".to_ascii_lowercase(),
                    PriceEvidence {
                        provider: Some(provider),
                        manual: None,
                    },
                )]),
            )]),
            global_manual_prices: BTreeMap::new(),
        };

        let source = context.candidate_price(&catalog, "source", "SOURCE", Some("gpt-test"));
        assert_eq!(source.source, PriceSource::Provider);
        assert_eq!(source.quote.unwrap(), provider);
        let account = context.candidate_price(&catalog, "account", "acct", Some("gpt-test"));
        assert_eq!(account.source, PriceSource::Unpriced);
    }
}

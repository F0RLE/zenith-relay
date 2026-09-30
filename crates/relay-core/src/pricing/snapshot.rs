use super::catalog::validate_litellm_payload;
use super::{ImageModelPrice, ImageRequestPrice, TokenPrice, MAX_CACHE_STRING_LENGTH};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub model_id: String,
    pub provider: Option<String>,
    pub token: Option<TokenPrice>,
    pub image: Option<ImageModelPrice>,
    pub request_micro_usd: Option<u64>,
}

impl CatalogEntry {
    pub(super) fn request_priced(
        model_id: String,
        provider: Option<String>,
        request_micro_usd: u64,
    ) -> Self {
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
    pub(super) fn equivalent_pricing(&self, other: &Self) -> bool {
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
    pub(super) unique: BTreeMap<String, CatalogEntry>,
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
            let Some(entry) = (match super::litellm_parser::parse_entry(model_id, value) {
                Ok(entry) => entry,
                Err(_) => continue,
            }) else {
                continue;
            };
            let key = super::normalize(model_id);
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

    /// Projects LiteLLM image fields into request-level rows. LiteLLM often
    /// publishes one generic image tariff without quality/size dimensions;
    /// those dimensions are represented as `default` rather than guessed.
    pub fn image_request_prices(&self, model: &str) -> Vec<ImageRequestPrice> {
        let normalized = super::normalize(model);
        let Some(entry) = self
            .entries
            .values()
            .filter(|entry| {
                !self.conflicts.contains(&super::normalize(&entry.model_id))
                    && (super::normalize(&entry.model_id) == normalized
                        || super::unqualified(&entry.model_id) == normalized)
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
                && !self.conflicts.contains(&super::normalize(&entry.model_id))
                && (super::normalize(&entry.model_id) == super::normalize(model)
                    || super::unqualified(&entry.model_id) == super::normalize(model))
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

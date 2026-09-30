mod capabilities;
mod catalog;
mod loader;
mod order;
mod parsing;
mod reasoning;
mod reference;
pub(crate) use reasoning::enrich_reasoning_metadata_with_models_dev_details;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

pub use loader::{ModelMetadataCatalogLoader, ModelMetadataError};
use parsing::{payload_hash, validate_payload};

pub const MODELS_DEV_SOURCE_URL: &str = "https://models.dev/models.json";
pub const MODELS_DEV_DETAILS_SOURCE_URL: &str = "https://models.dev/api.json";
pub const OPENROUTER_MODELS_SOURCE_URL: &str = "https://openrouter.ai/api/v1/models";
pub const LITELLM_MODELS_SOURCE_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const LEGACY_CACHE_FORMAT: &str = "zenith-relay-model-metadata-cache";
const CACHE_SCHEMA_VERSION: u32 = 1;
const MAX_RECORDS: usize = 100_000;
const MAX_STRING_LENGTH: usize = 1_024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub reasoning_source: Option<String>,
    #[serde(default)]
    pub reasoning_method: Option<ReasoningMethod>,
    #[serde(default)]
    pub reasoning_budget_min_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_budget_max_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_budget_default_tokens: Option<u64>,
    /// Provider/model-wide reasoning levels from the refreshed metadata
    /// catalog. These are advertised capabilities, not route verification.
    #[serde(
        default,
        alias = "reasoning_effort_levels",
        alias = "reasoningEffortLevels"
    )]
    pub reasoning_effort_levels: Vec<String>,
    #[serde(
        default,
        alias = "default_reasoning_effort",
        alias = "defaultReasoningEffort"
    )]
    pub default_reasoning_effort: Option<String>,
    pub tool_call: Option<bool>,
    pub structured_output: Option<bool>,
    pub attachment: Option<bool>,
    pub open_weights: Option<bool>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
    pub context_limit: Option<u64>,
    pub input_limit: Option<u64>,
    pub output_limit: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMethod {
    Effort,
    Toggle,
    BudgetTokens,
    Adaptive,
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMetadata {
    pub provider: String,
    pub family: Option<String>,
    pub name: Option<String>,
    pub release_date: Option<String>,
    pub last_updated: Option<String>,
    pub status: Option<String>,
    pub knowledge: Option<String>,
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMetadataCatalog {
    pub revision: Option<String>,
    pub fetched_at_ms: Option<u64>,
    pub stale: bool,
    pub sources: BTreeMap<String, MetadataSourceStatus>,
    entries: BTreeMap<String, ModelMetadata>,
    leaf_matches: BTreeMap<String, String>,
    ambiguous_leaves: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataSourceStatus {
    pub revision: Option<String>,
    pub fetched_at_ms: Option<u64>,
    pub stale: bool,
}

#[derive(Clone, Debug)]
pub struct ModelMetadataCatalogHandle {
    current: Arc<RwLock<Arc<ModelMetadataCatalog>>>,
}

impl ModelMetadataCatalogHandle {
    /// Share an already validated reference snapshot with a runtime.
    pub fn new(catalog: ModelMetadataCatalog) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(catalog))),
        }
    }

    pub fn snapshot(&self) -> Arc<ModelMetadataCatalog> {
        self.current
            .read()
            .expect("model metadata catalog lock poisoned")
            .clone()
    }

    pub(crate) fn replace(&self, catalog: ModelMetadataCatalog) {
        *self
            .current
            .write()
            .expect("model metadata catalog lock poisoned") = Arc::new(catalog);
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MetadataCacheEnvelope {
    pub(crate) format: String,
    pub(crate) schema_version: u32,
    pub(crate) source_url: String,
    pub(crate) revision: String,
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
    pub(crate) fetched_at_ms: u64,
    pub(crate) payload_sha256: String,
    pub(crate) stale: bool,
    pub(crate) payload: Value,
}

impl MetadataCacheEnvelope {
    #[cfg(test)]
    pub(crate) fn new(payload: Value, fetched_at_ms: u64) -> Result<Self, ModelMetadataError> {
        let revision = parsing::payload_hash(&payload)?;
        let envelope = Self {
            format: LEGACY_CACHE_FORMAT.to_string(),
            schema_version: CACHE_SCHEMA_VERSION,
            source_url: MODELS_DEV_SOURCE_URL.to_string(),
            revision: revision.clone(),
            etag: None,
            last_modified: None,
            fetched_at_ms,
            payload_sha256: revision,
            stale: false,
            payload,
        };
        envelope.validate()?;
        ModelMetadataCatalog::from_payload(&envelope.payload, None, None, false)?;
        Ok(envelope)
    }

    pub(crate) fn validate(&self) -> Result<(), ModelMetadataError> {
        if self.format != LEGACY_CACHE_FORMAT
            || self.schema_version != CACHE_SCHEMA_VERSION
            || self.source_url != MODELS_DEV_SOURCE_URL
            || self.fetched_at_ms == 0
            || self.revision != self.payload_sha256
            || self.revision != parsing::payload_hash(&self.payload)?
        {
            return Err(ModelMetadataError::InvalidCache);
        }
        parsing::validate_payload(&self.payload).map_err(|_| ModelMetadataError::InvalidCache)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

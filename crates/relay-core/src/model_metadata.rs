mod capabilities;
mod catalog;
mod display_projection;
mod export;
mod loader;
mod memory;
mod order;
mod parsing;
mod protocol_policy;
mod provenance;
mod reasoning;
mod reference;
pub(crate) use reasoning::enrich_reasoning_metadata_with_models_dev_details;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

pub(crate) use display_projection::DisplayProjection;
pub use export::{MODEL_CATALOG_SCHEMA, PublicModelCatalog, PublicModelCatalogEntry};
pub use loader::{ModelMetadataCatalogLoader, ModelMetadataError};
pub use order::{
    MODEL_ORDER_JSON, ModelDisplayIdentity, model_order_revision, validate_model_order_catalog,
};
use parsing::{metadata_payload_hash, validate_metadata_payload};

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
    pub temperature: Option<bool>,
    #[serde(default)]
    pub prompt_caching: Option<bool>,
    #[serde(default)]
    pub parallel_tool_call: Option<bool>,
    #[serde(default)]
    pub tool_choice: Option<bool>,
    #[serde(default)]
    pub streaming: Option<bool>,
    /// Explicit service modes in a trusted reference. Prices and model names
    /// never populate this list; a transport may narrow it further.
    #[serde(default)]
    pub service_tiers: Vec<crate::DefaultServiceTier>,
    /// Known reasoning modes are metadata, not arbitrary provider-body presets.
    #[serde(default)]
    pub reasoning_modes: Vec<String>,
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
    /// The identifier supplied by the reference source. Keep it separate
    /// from a hosted route ID or a canonical provider model ID.
    #[serde(default)]
    pub source_model_id: String,
    pub provider: String,
    /// A registry-provided relation to the underlying provider model. This is
    /// advisory identity metadata and never grants a runtime route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_model_id: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Ordered recommendations. They never declare a user's endpoint support.
    #[serde(default)]
    pub protocol_preferences: Vec<crate::WireApi>,
    #[serde(default)]
    pub field_provenance: BTreeMap<Cow<'static, str>, MetadataFieldProvenance>,
    pub family: Option<String>,
    pub name: Option<String>,
    pub release_date: Option<String>,
    pub last_updated: Option<String>,
    pub status: Option<String>,
    pub knowledge: Option<String>,
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFieldProvenance {
    #[serde(deserialize_with = "provenance::deserialize_source")]
    pub source: Cow<'static, str>,
    pub revision: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<MetadataFieldConflict>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFieldConflict {
    #[serde(deserialize_with = "provenance::deserialize_source")]
    pub source: Cow<'static, str>,
    pub revision: Option<Arc<str>>,
    pub value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMetadataCatalog {
    pub revision: Option<String>,
    pub fetched_at_ms: Option<u64>,
    pub stale: bool,
    pub sources: BTreeMap<String, MetadataSourceStatus>,
    entries: Arc<BTreeMap<String, ModelMetadata>>,
    leaf_matches: Arc<BTreeMap<String, String>>,
    ambiguous_leaves: Arc<BTreeSet<String>>,
    alias_matches: Arc<BTreeMap<String, Option<String>>>,
    /// None marks a family claimed by several primary catalog providers.
    family_providers: Arc<BTreeMap<String, Option<String>>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataSourceStatus {
    pub revision: Option<String>,
    pub fetched_at_ms: Option<u64>,
    pub stale: bool,
}

#[derive(Clone, Debug)]
pub struct ModelMetadataCatalogHandle {
    active_catalog: Arc<RwLock<Arc<ModelMetadataCatalog>>>,
}

impl ModelMetadataCatalogHandle {
    /// Share an already validated reference snapshot with a runtime.
    pub fn new(catalog: ModelMetadataCatalog) -> Self {
        Self {
            active_catalog: Arc::new(RwLock::new(Arc::new(catalog))),
        }
    }

    pub fn snapshot(&self) -> Arc<ModelMetadataCatalog> {
        self.active_catalog
            .read()
            .expect("model metadata catalog lock poisoned")
            .clone()
    }

    pub(crate) fn replace(&self, catalog: ModelMetadataCatalog) {
        *self
            .active_catalog
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
    pub(crate) fn new(
        metadata_payload: Value,
        fetched_at_ms: u64,
    ) -> Result<Self, ModelMetadataError> {
        let revision = parsing::metadata_payload_hash(&metadata_payload)?;
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
            payload: metadata_payload,
        };
        envelope.validate()?;
        ModelMetadataCatalog::from_metadata_payload(&envelope.payload, None, None, false)?;
        Ok(envelope)
    }

    pub(crate) fn validate(&self) -> Result<(), ModelMetadataError> {
        if self.format != LEGACY_CACHE_FORMAT
            || self.schema_version != CACHE_SCHEMA_VERSION
            || self.source_url != MODELS_DEV_SOURCE_URL
            || self.fetched_at_ms == 0
            || self.revision != self.payload_sha256
            || self.revision != parsing::metadata_payload_hash(&self.payload)?
        {
            return Err(ModelMetadataError::InvalidCache);
        }
        parsing::validate_metadata_payload(&self.payload)
            .map_err(|_| ModelMetadataError::InvalidCache)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

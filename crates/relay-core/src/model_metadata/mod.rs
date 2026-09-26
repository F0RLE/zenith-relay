mod capabilities;
mod loader;
mod reasoning;
mod reference;
pub(crate) use reasoning::enrich_reasoning_metadata_with_models_dev_details;
use reasoning::{
    normalize_external_levels, parse_effort_flags, parse_reasoning_object, parse_reasoning_options,
};

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

pub use loader::{ModelMetadataCatalogLoader, ModelMetadataError};

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

#[derive(Clone, Debug, Eq, PartialEq)]
struct FamilyOrder {
    newest_release: Option<u32>,
    catalog_release: Option<u32>,
    generation: Option<ModelGeneration>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ModelGeneration {
    prefix: String,
    version: Vec<u32>,
}

impl ModelMetadataCatalog {
    /// Presentation only: resolving a label must never rewrite the route ID or
    /// promote catalog metadata into an account's native transport contract.
    pub fn codex_display_name(&self, model: &str) -> String {
        self.resolve(model)
            .and_then(|metadata| metadata.name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .map(str::to_owned)
            .unwrap_or_else(|| crate::codex_model_display_name(model))
    }

    pub fn reasoning_levels_for(&self, model: &str) -> Vec<String> {
        let capabilities = self.capabilities_for(model);
        crate::canonicalize_reasoning_levels(capabilities.reasoning_effort_levels)
    }

    /// Capabilities belong to the exact model identity, not the provider route.
    /// Missing fields use Relay's shared baseline; source claims are not used.
    pub fn capabilities_for(&self, model: &str) -> ModelCapabilities {
        self.resolve(model)
            .map(|metadata| metadata.capabilities.clone().with_defaults())
            .unwrap_or_else(ModelCapabilities::unknown_model)
    }

    pub fn apply_codex_capabilities(&self, model: &str, entry: &mut Value) {
        self.capabilities_for(model).apply_to_codex(entry);
        crate::catalog::set_codex_service_tiers(entry, self.service_tiers_for(model));
    }

    pub fn service_tiers_for(&self, model: &str) -> &'static [crate::DefaultServiceTier] {
        crate::catalog::model_service_tiers(
            model,
            self.resolve(model)
                .map(|metadata| metadata.provider.as_str()),
        )
    }

    pub fn empty() -> Self {
        Self {
            revision: None,
            fetched_at_ms: None,
            stale: false,
            sources: BTreeMap::new(),
            entries: BTreeMap::new(),
            leaf_matches: BTreeMap::new(),
            ambiguous_leaves: BTreeSet::new(),
        }
    }

    pub fn from_models_dev_json(raw: &str) -> Result<Self, ModelMetadataError> {
        let payload = serde_json::from_str(raw).map_err(|_| ModelMetadataError::InvalidCatalog)?;
        Self::from_payload(&payload, None, None, false)
    }

    fn from_payload(
        payload: &Value,
        revision: Option<String>,
        fetched_at_ms: Option<u64>,
        stale: bool,
    ) -> Result<Self, ModelMetadataError> {
        let records = validate_payload(payload)?;
        let mut entries = BTreeMap::new();
        let mut leaf_matches = BTreeMap::new();
        let mut ambiguous_leaves = BTreeSet::new();
        let mut leaf_priorities = BTreeMap::new();

        for (source_id, value) in records {
            let provider = source_id
                .split_once('/')
                .map_or("", |(provider, _)| provider);
            let Some(metadata) = parse_metadata(provider, value) else {
                continue;
            };
            let key = normalize(&source_id);
            let leaf = model_leaf(&key).to_string();
            entries.insert(key.clone(), metadata);

            // Supplemental hosting catalogs must not make a canonical model
            // lose its unqualified identity. Equal-priority collisions remain
            // ambiguous; exact qualified IDs always resolve independently.
            let priority = reference::identity_priority(value);
            match leaf_priorities.get(&leaf) {
                Some(previous) if *previous < priority => continue,
                Some(previous) if *previous == priority => {}
                _ => {
                    leaf_priorities.insert(leaf.clone(), priority);
                    ambiguous_leaves.remove(&leaf);
                    leaf_matches.insert(leaf, key);
                    continue;
                }
            }
            if ambiguous_leaves.contains(&leaf) {
                continue;
            }
            if let Some(previous) = leaf_matches.insert(leaf.clone(), key.clone()) {
                if previous != key {
                    leaf_matches.remove(&leaf);
                    ambiguous_leaves.insert(leaf);
                }
            }
        }

        if entries.is_empty() {
            return Err(ModelMetadataError::InvalidCatalog);
        }

        Ok(Self {
            revision,
            fetched_at_ms,
            stale,
            sources: BTreeMap::new(),
            entries,
            leaf_matches,
            ambiguous_leaves,
        })
    }

    /// Resolve exact catalog IDs first. An unqualified or Relay-qualified ID
    /// may use its leaf only when that leaf identifies one catalog record.
    pub fn resolve(&self, model: &str) -> Option<&ModelMetadata> {
        let key = normalize(model);
        if let Some(metadata) = self.entries.get(&key) {
            return Some(metadata);
        }
        let leaf = model_leaf(&key);
        if self.ambiguous_leaves.contains(leaf) {
            return None;
        }
        self.leaf_matches
            .get(leaf)
            .and_then(|id| self.entries.get(id))
    }

    pub fn reasoning_effort_levels(&self, model: &str) -> Option<Vec<String>> {
        self.resolve(model)
            .map(|metadata| metadata.capabilities.reasoning_effort_levels.clone())
    }

    /// Keep companies and catalog families together. Release/update dates order
    /// versions inside each family; presentation never determines eligibility.
    pub fn order_model_ids<I, S>(&self, models: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let source = crate::normalize_model_ids(models);
        let family_order = self.family_order(&source);
        let mut indexed = source.into_iter().collect::<Vec<_>>();
        indexed.sort_by(|left_id, right_id| {
            compare_metadata(self.resolve(left_id), self.resolve(right_id), &family_order)
                .then_with(|| normalize(left_id).cmp(&normalize(right_id)))
                .then_with(|| left_id.cmp(right_id))
        });
        indexed
    }

    /// Preserve the relative order explicitly saved by the user. Newly
    /// discovered models are inserted at their catalog position around those
    /// anchors instead of resetting the complete list.
    pub fn merge_display_order<I, S>(&self, models: I, saved_order: &[String]) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let source = crate::normalize_model_ids(models);
        let available = source
            .iter()
            .map(|id| normalize(id))
            .collect::<BTreeSet<_>>();
        let catalog_order = self.order_model_ids(source);
        if !saved_order
            .iter()
            .any(|id| available.contains(&normalize(id)))
        {
            return catalog_order;
        }

        let positions = catalog_order
            .iter()
            .enumerate()
            .map(|(position, id)| (normalize(id), position))
            .collect::<BTreeMap<_, _>>();
        let mut saved = BTreeSet::new();
        let mut ordered = Vec::with_capacity(catalog_order.len());

        for id in saved_order {
            let key = normalize(id);
            if saved.insert(key.clone()) {
                if let Some(position) = positions.get(&key) {
                    ordered.push(catalog_order[*position].clone());
                }
            }
        }
        for id in catalog_order {
            let key = normalize(&id);
            if saved.contains(&key) {
                continue;
            }
            let position = positions[&key];
            let insertion = ordered
                .iter()
                .position(|existing| positions[&normalize(existing)] > position)
                .unwrap_or(ordered.len());
            ordered.insert(insertion, id);
        }
        ordered
    }

    fn family_order(&self, models: &[String]) -> BTreeMap<(String, String), FamilyOrder> {
        let mut families = BTreeMap::new();
        for id in models {
            let Some(metadata) = self.resolve(id) else {
                continue;
            };
            let Some(key) = provider_key(metadata) else {
                continue;
            };
            let Some(family) = family_key(metadata) else {
                continue;
            };
            let release = metadata.release_date.as_deref().and_then(date_key);
            let generation = model_generation(id);
            families
                .entry((key, family))
                .and_modify(|order: &mut FamilyOrder| {
                    order.newest_release = order.newest_release.max(release);
                    order.catalog_release = order.newest_release;
                    merge_generation(&mut order.generation, generation.clone());
                })
                .or_insert(FamilyOrder {
                    newest_release: release,
                    catalog_release: release,
                    generation,
                });
        }

        // Sibling families in one numbered model generation form a catalog
        // cohort. Rank that cohort by its newest release, then order its
        // variants by their stable metadata family IDs. A newer sibling such
        // as GPT-6 Sol must not leapfrog GPT-6 Astra just because it launched
        // later.
        let mut cohort_releases = BTreeMap::new();
        for ((provider, _), order) in &families {
            let Some(generation) = &order.generation else {
                continue;
            };
            cohort_releases
                .entry((provider.clone(), generation.clone()))
                .and_modify(|release: &mut Option<u32>| {
                    *release = (*release).max(order.newest_release);
                })
                .or_insert(order.newest_release);
        }
        for ((provider, _), order) in &mut families {
            if let Some(generation) = &order.generation {
                order.catalog_release = cohort_releases
                    .get(&(provider.clone(), generation.clone()))
                    .copied()
                    .unwrap_or(order.newest_release);
            }
        }
        families
    }
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
        let revision = payload_hash(&payload)?;
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
            || self.revision != payload_hash(&self.payload)?
        {
            return Err(ModelMetadataError::InvalidCache);
        }
        validate_payload(&self.payload).map_err(|_| ModelMetadataError::InvalidCache)?;
        Ok(())
    }
}

fn parse_metadata(provider: &str, value: &Value) -> Option<ModelMetadata> {
    let object = value.as_object()?;
    let string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && value.len() <= MAX_STRING_LENGTH)
            .map(str::to_string)
    };
    let boolean = |key: &str| object.get(key).and_then(Value::as_bool);
    let modalities = object.get("modalities").and_then(Value::as_object);
    let architecture = object.get("architecture").and_then(Value::as_object);
    let limits = object.get("limit").and_then(Value::as_object);
    let supported_parameters = string_array(object.get("supported_parameters"));
    let reasoning_options = parse_reasoning_options(
        object
            .get("reasoning_options")
            .or_else(|| object.get("reasoningOptions")),
    );
    let reasoning_method = object
        .get("reasoning_method")
        .and_then(Value::as_str)
        .and_then(|method| match method {
            "effort" => Some(ReasoningMethod::Effort),
            "toggle" => Some(ReasoningMethod::Toggle),
            "budget_tokens" => Some(ReasoningMethod::BudgetTokens),
            "adaptive" => Some(ReasoningMethod::Adaptive),
            "unknown" => Some(ReasoningMethod::Unknown),
            _ => None,
        })
        .or_else(|| parse_reasoning_object(object.get("reasoning")).method)
        .or_else(|| reasoning_options.method.clone())
        .or_else(|| {
            if supported_parameters.iter().any(|p| p == "reasoning_effort") {
                Some(ReasoningMethod::Effort)
            } else if supported_parameters.iter().any(|p| p == "reasoning") {
                Some(ReasoningMethod::Unknown)
            } else {
                None
            }
        });
    // `name` is descriptive only. Keep a record when the upstream catalog
    // omits it so family, dates, and capability metadata still apply.
    let name = string("name");

    let direct_or_supported = |key: &str, supported: &str| {
        boolean(key).or_else(|| {
            (!supported_parameters.is_empty())
                .then(|| supported_parameters.iter().any(|value| value == supported))
        })
    };

    let input_modalities = string_array(
        modalities
            .and_then(|value| value.get("input"))
            .or_else(|| architecture.and_then(|value| value.get("input_modalities"))),
    );
    let output_modalities = string_array(
        modalities
            .and_then(|value| value.get("output"))
            .or_else(|| architecture.and_then(|value| value.get("output_modalities"))),
    );
    let mut reasoning_effort_levels = string_array(
        object
            .get("reasoning_effort_levels")
            .or_else(|| object.get("reasoningEffortLevels")),
    );
    if reasoning_effort_levels.is_empty() {
        reasoning_effort_levels = parse_effort_flags(object);
    }
    if reasoning_effort_levels.is_empty() {
        reasoning_effort_levels = reasoning_options.levels.clone();
    }
    reasoning_effort_levels = normalize_external_levels(reasoning_effort_levels);

    Some(ModelMetadata {
        provider: provider.to_string(),
        family: string("family").or_else(|| string("model_family")),
        name,
        release_date: string("release_date").filter(|date| date_key(date).is_some()),
        last_updated: string("last_updated").filter(|date| date_key(date).is_some()),
        status: string("status"),
        knowledge: string("knowledge").or_else(|| string("knowledge_cutoff")),
        capabilities: ModelCapabilities {
            reasoning: direct_or_supported("reasoning", "reasoning")
                .or(reasoning_options.supported),
            reasoning_source: string("reasoning_source"),
            reasoning_method,
            reasoning_budget_min_tokens: object
                .get("reasoning_budget_min_tokens")
                .and_then(Value::as_u64),
            reasoning_budget_max_tokens: object
                .get("reasoning_budget_max_tokens")
                .and_then(Value::as_u64),
            reasoning_budget_default_tokens: object
                .get("reasoning_budget_default_tokens")
                .and_then(Value::as_u64),
            reasoning_effort_levels,
            default_reasoning_effort: string("default_reasoning_effort")
                .or_else(|| string("defaultReasoningEffort")),
            tool_call: direct_or_supported("tool_call", "tools"),
            structured_output: direct_or_supported("structured_output", "structured_outputs")
                .or_else(|| {
                    (!supported_parameters.is_empty()).then(|| {
                        supported_parameters
                            .iter()
                            .any(|value| value == "response_format")
                    })
                }),
            attachment: boolean("attachment"),
            open_weights: boolean("open_weights"),
            input_modalities,
            output_modalities,
            context_limit: limits
                .and_then(|value| value.get("context"))
                .and_then(Value::as_u64)
                .or_else(|| object.get("context_length").and_then(Value::as_u64)),
            input_limit: limits
                .and_then(|value| value.get("input"))
                .and_then(Value::as_u64),
            output_limit: limits
                .and_then(|value| value.get("output"))
                .and_then(Value::as_u64),
        },
    })
}

fn compare_metadata(
    left: Option<&ModelMetadata>,
    right: Option<&ModelMetadata>,
    family_order: &BTreeMap<(String, String), FamilyOrder>,
) -> Ordering {
    let left_provider = left.and_then(provider_key);
    let right_provider = right.and_then(provider_key);

    match (left_provider.as_ref(), right_provider.as_ref()) {
        (Some(left_key), Some(right_key)) if left_key == right_key => {
            compare_families(left_key, left, right, family_order)
                .then_with(|| compare_model_dates(left, right))
        }
        (Some(left_key), Some(right_key)) => company_order(left_key)
            .cmp(&company_order(right_key))
            .then_with(|| left_key.cmp(right_key)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn company_order(provider: &str) -> usize {
    match provider {
        "openai" => 0,
        "anthropic" => 1,
        "google" => 2,
        "xai" | "x-ai" => 3,
        _ => 4,
    }
}

fn family_key(metadata: &ModelMetadata) -> Option<String> {
    metadata
        .family
        .as_deref()
        .map(normalize)
        .filter(|family| !family.is_empty())
}

fn compare_families(
    provider: &str,
    left: Option<&ModelMetadata>,
    right: Option<&ModelMetadata>,
    families: &BTreeMap<(String, String), FamilyOrder>,
) -> Ordering {
    match (left.and_then(family_key), right.and_then(family_key)) {
        (Some(left), Some(right)) if left != right => {
            let left_order = &families[&(provider.to_string(), left.clone())];
            let right_order = &families[&(provider.to_string(), right.clone())];
            compare_optional_date_desc(left_order.catalog_release, right_order.catalog_release)
                .then_with(|| left.cmp(&right))
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn merge_generation(current: &mut Option<ModelGeneration>, incoming: Option<ModelGeneration>) {
    match (current.as_ref(), incoming) {
        (Some(existing), Some(incoming)) if existing.prefix == incoming.prefix => {
            if compare_generation_version(&incoming.version, &existing.version) == Ordering::Greater
            {
                *current = Some(incoming);
            }
        }
        (None, None) => {}
        _ => *current = None,
    }
}

fn model_generation(id: &str) -> Option<ModelGeneration> {
    let normalized = normalize(id);
    let leaf = model_leaf(&normalized);
    let start = leaf.find(|character: char| character.is_ascii_digit())?;
    let prefix = leaf[..start].trim_end_matches(['-', '_', '.']);
    if prefix.is_empty() {
        return None;
    }

    let version_end = leaf[start..]
        .char_indices()
        .take_while(|(offset, character)| {
            character.is_ascii_digit()
                || (*character == '.'
                    && leaf[start + offset + 1..]
                        .chars()
                        .next()
                        .is_some_and(|next| next.is_ascii_digit()))
        })
        .map(|(offset, character)| offset + character.len_utf8())
        .last()?;
    let version = leaf[start..start + version_end]
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!version.is_empty()).then(|| ModelGeneration {
        prefix: prefix.to_owned(),
        version,
    })
}

fn compare_generation_version(left: &[u32], right: &[u32]) -> Ordering {
    let width = left.len().max(right.len());
    (0..width)
        .map(|index| {
            left.get(index)
                .copied()
                .unwrap_or_default()
                .cmp(&right.get(index).copied().unwrap_or_default())
        })
        .find(|ordering| *ordering != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

fn compare_model_dates(left: Option<&ModelMetadata>, right: Option<&ModelMetadata>) -> Ordering {
    compare_optional_date_desc(
        metadata_date(left, |metadata| metadata.release_date.as_deref()),
        metadata_date(right, |metadata| metadata.release_date.as_deref()),
    )
    .then_with(|| {
        compare_optional_date_desc(
            metadata_date(left, |metadata| metadata.last_updated.as_deref()),
            metadata_date(right, |metadata| metadata.last_updated.as_deref()),
        )
    })
}

fn compare_optional_date_desc(left: Option<u32>, right: Option<u32>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn metadata_date(
    metadata: Option<&ModelMetadata>,
    field: impl FnOnce(&ModelMetadata) -> Option<&str>,
) -> Option<u32> {
    metadata.and_then(field).and_then(date_key)
}

fn provider_key(metadata: &ModelMetadata) -> Option<String> {
    let provider = normalize(&metadata.provider);
    (!provider.is_empty()).then_some(provider)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_STRING_LENGTH)
        .map(str::to_string)
        .collect()
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn model_leaf(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn date_key(value: &str) -> Option<u32> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            let (year, month) = value.split_once('-')?;
            if month.len() != 2 {
                return None;
            }
            NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)
        })?;
    let year = u32::try_from(date.year()).ok()?;
    Some(year.saturating_mul(10_000) + date.month() * 100 + date.day())
}

fn validate_payload(payload: &Value) -> Result<Vec<(String, &Value)>, ModelMetadataError> {
    let mut records = Vec::new();

    if let Some(object) = payload.as_object() {
        if let Some(data) = object.get("data") {
            let array = data.as_array().ok_or(ModelMetadataError::InvalidCatalog)?;
            for value in array {
                let record = value
                    .as_object()
                    .ok_or(ModelMetadataError::InvalidCatalog)?;
                let id = record
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty() && id.len() <= MAX_STRING_LENGTH)
                    .ok_or(ModelMetadataError::InvalidCatalog)?;
                records.push((id.to_string(), value));
            }
        } else {
            for (id, value) in object {
                if id.trim().is_empty() || id.len() > MAX_STRING_LENGTH {
                    return Err(ModelMetadataError::InvalidCatalog);
                }
                if value.as_object().is_none() {
                    return Err(ModelMetadataError::InvalidCatalog);
                }
                records.push((id.clone(), value));
            }
        }
    } else if let Some(array) = payload.as_array() {
        for value in array {
            let record = value
                .as_object()
                .ok_or(ModelMetadataError::InvalidCatalog)?;
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty() && id.len() <= MAX_STRING_LENGTH)
                .ok_or(ModelMetadataError::InvalidCatalog)?;
            records.push((id.to_string(), value));
        }
    } else {
        return Err(ModelMetadataError::InvalidCatalog);
    }

    if records.is_empty() || records.len() > MAX_RECORDS {
        return Err(ModelMetadataError::InvalidCatalog);
    }
    Ok(records)
}

fn payload_hash(payload: &Value) -> Result<String, ModelMetadataError> {
    let bytes = serde_json::to_vec(payload).map_err(|_| ModelMetadataError::InvalidCache)?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_metadata::reasoning::enrich_reasoning_metadata;

    #[test]
    fn merges_openrouter_reasoning_levels_over_litellm_and_models_dev() {
        let models = serde_json::json!({
            "openai/gpt-test": {"reasoning": true}
        });
        let openrouter = serde_json::json!({"data": [{
            "id": "openai/gpt-test",
            "supported_parameters": ["reasoning", "reasoning_effort"],
            "reasoning": {"effort": ["low", "high"]}
        }]});
        let litellm = serde_json::json!({
            "openai/gpt-test": {
                "supports_low_reasoning_effort": true,
                "supports_medium_reasoning_effort": true,
                "supports_high_reasoning_effort": true
            }
        });
        let merged = enrich_reasoning_metadata(&models, Some(&openrouter), Some(&litellm));
        assert_eq!(
            merged["openai/gpt-test"]["reasoning_effort_levels"],
            serde_json::json!(["low", "high"])
        );
    }

    #[test]
    fn falls_back_to_litellm_when_openrouter_has_no_exact_levels() {
        let models = serde_json::json!({"openai/gpt-test": {"reasoning": true}});
        let openrouter = serde_json::json!({"data": [{
            "id": "openai/gpt-test",
            "supported_parameters": ["reasoning_effort"]
        }]});
        let litellm = serde_json::json!({
            "openai/gpt-test": {
                "supports_minimal_reasoning_effort": true,
                "supports_high_reasoning_effort": true
            }
        });
        let merged = enrich_reasoning_metadata(&models, Some(&openrouter), Some(&litellm));
        assert_eq!(
            merged["openai/gpt-test"]["reasoning_effort_levels"],
            serde_json::json!(["minimal", "high"])
        );
    }

    #[test]
    fn does_not_invent_levels_from_reasoning_boolean() {
        let models = serde_json::json!({"openai/gpt-test": {"reasoning": true}});
        let merged = enrich_reasoning_metadata(&models, None, None);
        assert!(merged["openai/gpt-test"]
            .get("reasoning_effort_levels")
            .is_none());
    }

    #[test]
    fn parses_reasoning_method_and_filters_unrecognized_efforts() {
        let models = serde_json::json!({"provider/model": {"reasoning": true}});
        let openrouter = serde_json::json!({"data": [{
            "id": "provider/model",
            "reasoning": {"type": "effort", "effort": {"values": ["low", "HIGH", "vendor-private", "x".repeat(2_000)]}, "default_effort": "high"},
            "supported_parameters": ["reasoning"]
        }]});
        let catalog = ModelMetadataCatalog::from_payload(
            &enrich_reasoning_metadata(&models, Some(&openrouter), None),
            None,
            None,
            false,
        )
        .unwrap();
        let capabilities = catalog.capabilities_for("provider/model");
        assert_eq!(capabilities.reasoning_method, Some(ReasoningMethod::Effort));
        assert_eq!(capabilities.reasoning_effort_levels, ["low", "high"]);
        assert_eq!(capabilities.default_reasoning_effort, Some("high".into()));
    }

    #[test]
    fn parses_models_dev_reasoning_options() {
        let catalog = catalog(
            r#"{
            "xai/grok-4.6": {
                "name": "Grok 4.6",
                "reasoning": true,
                "reasoning_options": [
                    {"type": "effort", "values": ["low", "medium", "high", "xhigh"]}
                ]
            }
        }"#,
        );
        let capabilities = catalog.capabilities_for("x-ai/grok-4.6");
        assert_eq!(capabilities.reasoning, Some(true));
        assert_eq!(
            capabilities.reasoning_effort_levels,
            ["low", "medium", "high", "xhigh"]
        );
    }

    #[test]
    fn enriches_models_dev_records_from_nested_api_details() {
        let models = serde_json::json!({
            "xai/grok-4.6": {"reasoning": true}
        });
        let details = serde_json::json!({
            "xai": {"models": {
                "grok-4.6": {
                    "id": "grok-4.6",
                    "reasoning": true,
                    "reasoning_options": [{
                        "type": "effort",
                        "values": ["low", "medium", "high", "xhigh"]
                    }]
                }
            }}
        });
        let merged = reasoning::enrich_reasoning_metadata_with_models_dev_details(
            &models,
            Some(&details),
            None,
            None,
        );
        assert_eq!(
            merged["xai/grok-4.6"]["reasoning_effort_levels"],
            serde_json::json!(["low", "medium", "high", "xhigh"])
        );
        assert_eq!(
            merged["xai/grok-4.6"]["reasoning_source"],
            serde_json::json!("models_dev")
        );
    }

    #[test]
    fn preserves_details_reasoning_levels_when_options_describe_another_control() {
        let models = serde_json::json!({
            "vendor/model": {"reasoning": true}
        });
        let details = serde_json::json!({
            "vendor": {"models": {
                "model": {
                    "id": "model",
                    "reasoning": {
                        "type": "effort",
                        "supported_efforts": ["low", "high"]
                    },
                    "reasoning_options": [{
                        "type": "toggle",
                        "enabled": true
                    }]
                }
            }}
        });

        let merged = reasoning::enrich_reasoning_metadata_with_models_dev_details(
            &models,
            Some(&details),
            None,
            None,
        );

        assert_eq!(
            merged["vendor/model"]["reasoning_effort_levels"],
            serde_json::json!(["low", "high"])
        );
    }

    #[test]
    fn recognizes_litellm_camel_case_effort_flags_without_openrouter() {
        let models = serde_json::json!({"provider/model": {"reasoning": true}});
        let litellm = serde_json::json!({"provider/model": {
            "supportsLowReasoningEffort": true,
            "supportsHighReasoningEffort": true
        }});
        let catalog = ModelMetadataCatalog::from_payload(
            &enrich_reasoning_metadata(&models, None, Some(&litellm)),
            None,
            None,
            false,
        )
        .unwrap();
        let capabilities = catalog.capabilities_for("provider/model");
        assert_eq!(capabilities.reasoning_method, Some(ReasoningMethod::Effort));
        assert_eq!(capabilities.reasoning_effort_levels, ["low", "high"]);
    }

    #[test]
    fn matches_decimal_and_dashed_model_versions_before_litellm_fallback() {
        let models = serde_json::json!({
            "anthropic/claude-fable-5-1": {"reasoning": true},
            "anthropic/claude-opus-4-8": {"reasoning": true}
        });
        let openrouter = serde_json::json!({"data": [
            {
                "id": "anthropic/claude-fable-5.1",
                "reasoning": {"supported_efforts": ["max", "xhigh", "high", "medium", "low"]}
            },
            {
                "id": "anthropic/claude-fable-5.1:batch",
                "reasoning": {"supported_efforts": ["max"]}
            },
            {
                "id": "anthropic/claude-opus-4.8",
                "reasoning": {"supported_efforts": ["max", "xhigh", "high", "medium", "low"]}
            }
        ]});
        let litellm = serde_json::json!({
            "claude-fable-5-1": {
                "supports_xhigh_reasoning_effort": true,
                "supports_max_reasoning_effort": true
            },
            "claude-opus-4-8": {"supports_max_reasoning_effort": true}
        });

        let catalog = ModelMetadataCatalog::from_payload(
            &enrich_reasoning_metadata(&models, Some(&openrouter), Some(&litellm)),
            None,
            None,
            false,
        )
        .unwrap();

        assert_eq!(
            catalog.reasoning_levels_for("claude-fable-5-1"),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            catalog.reasoning_levels_for("claude-opus-4-8"),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            catalog
                .capabilities_for("claude-fable-5-1")
                .reasoning_source
                .as_deref(),
            Some("openrouter")
        );
    }

    fn catalog(raw: &str) -> ModelMetadataCatalog {
        ModelMetadataCatalog::from_models_dev_json(raw).unwrap()
    }

    #[test]
    fn resolves_exact_and_unique_leaf_ids() {
        let catalog = catalog(
            r#"{
                "openai/gpt-test":{"name":"GPT Test","family":"gpt","release_date":"2026-01-01"},
                "anthropic/claude-test":{"name":"Claude Test","family":"claude","release_date":"2025-01-01"}
            }"#,
        );
        assert_eq!(
            catalog
                .resolve("openai/gpt-test")
                .unwrap()
                .family
                .as_deref(),
            Some("gpt")
        );
        assert_eq!(
            catalog.resolve("relay/gpt-test").unwrap().provider,
            "openai"
        );
    }

    #[test]
    fn codex_labels_use_catalog_names_without_guessing_ambiguous_models() {
        let catalog = catalog(
            r#"{
            "openai/new-family": {"name": "Future Native Name"},
            "alpha/shared": {"name": "Alpha Name"},
            "beta/shared": {"name": "Beta Name"},
            "openai/invalid-name": {"name": "Bad\nName"},
            "openai/unnamed": {"family": "future"}
        }"#,
        );
        for (id, name) in [
            ("openai/new-family", "Future Native Name"),
            ("new-family", "Future Native Name"),
            ("relay/new-family", "Future Native Name"),
            ("alpha/shared", "Alpha Name"),
            ("beta/shared", "Beta Name"),
            ("shared", "Shared"),
            ("unrelated/shared", "Shared"),
            ("invalid-name", "Invalid Name"),
            ("unnamed", "Unnamed"),
            ("gpt-123-future", "123 Future"),
        ] {
            assert_eq!(catalog.codex_display_name(id), name, "{id}");
        }
    }

    #[test]
    fn accepts_models_dev_data_payload_and_maps_capabilities() {
        let catalog = catalog(
            r#"{
                "data": [
                    {
                        "id": "openai/gpt-array",
                        "name": "GPT Array",
                        "architecture": {
                            "input_modalities": ["text", "image"],
                            "output_modalities": ["text"]
                        },
                        "context_length": 128000,
                        "supported_parameters": ["reasoning", "structured_outputs", "tools"]
                    }
                ]
            }"#,
        );
        let metadata = catalog.resolve("relay/gpt-array").unwrap();
        assert_eq!(metadata.provider, "openai");
        assert_eq!(metadata.name.as_deref(), Some("GPT Array"));
        assert_eq!(metadata.capabilities.context_limit, Some(128000));
        assert_eq!(metadata.capabilities.input_modalities, ["text", "image"]);
        assert_eq!(metadata.capabilities.reasoning, Some(true));
        assert_eq!(metadata.capabilities.tool_call, Some(true));
        assert_eq!(metadata.capabilities.structured_output, Some(true));
    }

    #[test]
    fn keeps_records_without_a_descriptive_name() {
        let catalog = catalog(
            r#"{
                "openai/unnamed": {
                    "family": "gpt",
                    "release_date": "2026-01-01"
                }
            }"#,
        );
        let metadata = catalog.resolve("openai/unnamed").unwrap();
        assert_eq!(metadata.name, None);
        assert_eq!(metadata.family.as_deref(), Some("gpt"));
    }

    #[test]
    fn ambiguous_leaf_ids_are_not_guessed() {
        let catalog = catalog(
            r#"{
                "openai/shared":{"name":"OpenAI Shared","family":"gpt"},
                "anthropic/shared":{"name":"Anthropic Shared","family":"claude"}
            }"#,
        );
        assert!(catalog.resolve("shared").is_none());
        assert!(catalog.resolve("relay/shared").is_none());
        assert!(catalog.resolve("openai/shared").is_some());
    }

    #[test]
    fn default_company_order_preserves_manual_override_and_unknown_models() {
        let catalog = catalog(
            r#"{
            "xai/model-x":{"family":"line-x"},
            "google/model-g":{"family":"line-g"},
            "openai/model-o":{"family":"line-o"},
            "anthropic/model-a":{"family":"line-a"},
            "another/model-n":{"family":"line-n"}
        }"#,
        );
        let inventory = [
            "unknown", "model-x", "model-n", "model-a", "model-g", "model-o",
        ];
        assert_eq!(
            catalog.order_model_ids(inventory),
            ["model-o", "model-a", "model-g", "model-x", "model-n", "unknown"]
        );
        let manual = inventory.map(str::to_string);
        assert_eq!(catalog.merge_display_order(inventory, &manual), inventory);
        assert_eq!(
            catalog.merge_display_order(inventory, &[]),
            catalog.order_model_ids(inventory)
        );
    }
    #[test]
    fn order_uses_release_dates_and_keeps_unknown_models_last() {
        let catalog = catalog(
            r#"{
                "openai/old":{"name":"Old","family":"gpt","release_date":"2025-01-01"},
                "openai/new":{"name":"New","family":"gpt","release_date":"2026-01-01"}
            }"#,
        );
        assert_eq!(
            catalog.order_model_ids(["unknown", "old", "new"]),
            ["new", "old", "unknown"]
        );
    }

    #[test]
    fn same_generation_families_use_stable_family_order_not_release_date() {
        let catalog = catalog(
            r#"{
                "openai/gpt-6-sol":{"name":"GPT-6 Sol","family":"gpt-sol","release_date":"2026-09-22"},
                "openai/gpt-5.6-sol":{"name":"GPT-5.6 Sol","family":"gpt-sol","release_date":"2026-07-09"},
                "openai/gpt-6-astra":{"name":"GPT-6 Astra","family":"gpt-astra","release_date":"2026-09-04"},
                "openai/gpt-5.6-luna":{"name":"GPT-5.6 Luna","family":"gpt-luna","release_date":"2026-07-09"},
                "openai/gpt-5.6-terra":{"name":"GPT-5.6 Terra","family":"gpt-terra","release_date":"2026-07-09"}
            }"#,
        );
        let expected = [
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-5.6-sol",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
        ];
        assert_eq!(
            catalog.order_model_ids(expected.into_iter().rev()),
            expected
        );
        assert_eq!(catalog.order_model_ids(expected), expected);
    }

    #[test]
    fn company_blocks_do_not_merge_matching_family_names() {
        let catalog = catalog(
            r#"{
                "alpha/shared-new":{"name":"Shared New","family":"shared","release_date":"2026-01-01"},
                "alpha/shared-old":{"name":"Shared Old","family":"shared","release_date":"2024-01-01"},
                "beta/shared-model":{"name":"Shared Model","family":"shared","release_date":"2025-01-01"}
            }"#,
        );
        assert_eq!(
            catalog.order_model_ids(["shared-old", "shared-model", "shared-new"]),
            ["shared-new", "shared-old", "shared-model"]
        );
    }

    #[test]
    fn catalog_families_group_versions_for_every_company_without_name_rules() {
        let catalog = catalog(
            r#"{
                "anthropic/fable-new":{"family":"claude-fable","release_date":"2026-09-01"},
                "anthropic/fable-old":{"family":"claude-fable","release_date":"2026-06-09"},
                "anthropic/opus-new":{"family":"claude-opus","release_date":"2026-07-24"},
                "anthropic/opus-old":{"family":"claude-opus","release_date":"2026-02-05"},
                "anthropic/sonnet":{"family":"claude-sonnet","release_date":"2026-06-30"},
                "anthropic/haiku":{"family":"claude-haiku","release_date":"2025-10-15"},
                "google/flash-new":{"family":"gemini-flash","release_date":"2026-09-10"},
                "google/flash-old":{"family":"gemini-flash","release_date":"2025-01-01"},
                "google/pro":{"family":"gemini-pro","release_date":"2026-02-19"},
                "openai/full-new":{"family":"gpt","release_date":"2026-09-04"},
                "openai/full-old":{"family":"gpt","release_date":"2025-01-01"},
                "openai/mini":{"family":"gpt-mini","release_date":"2026-03-05"},
                "future-company/next-new":{"family":" NEW-LINE ","release_date":"2028-01-01"},
                "future-company/next-old":{"family":"new-line","release_date":"2024-01-01"},
                "future-company/mid":{"family":"other-line","release_date":"2027-01-01"}
            }"#,
        );
        let expected = [
            "full-new",
            "full-old",
            "mini",
            "fable-new",
            "fable-old",
            "opus-new",
            "opus-old",
            "sonnet",
            "haiku",
            "flash-new",
            "flash-old",
            "pro",
            "next-new",
            "next-old",
            "mid",
        ];
        assert_eq!(
            catalog.order_model_ids(expected.into_iter().rev()),
            expected
        );
        assert_eq!(
            catalog.merge_display_order(expected.into_iter().rev(), &[]),
            expected
        );
    }

    #[test]
    fn company_order_keeps_families_together_and_missing_families_last() {
        let catalog = catalog(
            r#"{
                "alpha/new":{"family":"large","release_date":"2026-06-01"},
                "alpha/old":{"family":"large","release_date":"2024-01-01"},
                "alpha/middle":{"family":"small","release_date":"2026-01-01"},
                "alpha/no-family":{"release_date":"2026-03-01"},
                "alpha/undated":{"family":"small"},
                "beta/other":{"family":"large","release_date":"2025-01-01"}
            }"#,
        );
        assert_eq!(
            catalog.order_model_ids([
                "unknown",
                "old",
                "other",
                "undated",
                "middle",
                "no-family",
                "new"
            ]),
            [
                "new",
                "old",
                "middle",
                "undated",
                "no-family",
                "other",
                "unknown"
            ]
        );
    }

    #[test]
    fn equal_dates_use_stable_family_and_model_ids() {
        let catalog = catalog(
            r#"{
                "alpha/first":{"family":"small","release_date":"2026-01-01"},
                "alpha/second":{"family":"large","release_date":"2026-01-01"},
                "alpha/third":{"family":"small","release_date":"2026-01-01"}
            }"#,
        );
        assert_eq!(
            catalog.order_model_ids(["first", "second", "third"]),
            ["second", "first", "third"]
        );
        assert_eq!(
            catalog.order_model_ids(["third", "first", "second"]),
            ["second", "first", "third"]
        );
    }

    #[test]
    fn invalid_or_missing_dates_use_stable_ids_and_unknown_models_stay_last() {
        let catalog = catalog(
            r#"{
                "alpha/first":{"name":"First","family":"alpha","release_date":"not-a-date"},
                "alpha/second":{"name":"Second","family":"alpha"}
            }"#,
        );
        assert_eq!(
            catalog.order_model_ids(["second", "first", "unknown"]),
            ["first", "second", "unknown"]
        );
        assert_eq!(
            catalog.order_model_ids(["unknown", "first", "second"]),
            ["first", "second", "unknown"]
        );
    }

    #[test]
    fn clearing_saved_order_restores_catalog_groups_and_newest_models_first() {
        let catalog = catalog(
            r#"{
                "anthropic/claude-old":{"release_date":"2025-10-15","last_updated":"2026-09-19"},
                "anthropic/claude-new":{"release_date":"2026-09-01"},
                "openai/gpt-old":{"release_date":"2025-01-01"},
                "openai/gpt-new":{"release_date":"2026-09-04"},
                "google/gemini-old":{"release_date":"2025-01-01"},
                "google/gemini-new":{"release_date":"2026-02-19"},
                "google/gemini-undated":{}
            }"#,
        );
        let inventory = [
            "claude-old",
            "gpt-old",
            "gemini-undated",
            "gemini-old",
            "claude-new",
            "gpt-new",
            "gemini-new",
            "unknown",
        ];
        let manual = inventory.map(str::to_string);
        assert_eq!(catalog.merge_display_order(inventory, &manual), manual);
        let reset = catalog.merge_display_order(inventory, &[]);
        assert_eq!(
            reset,
            [
                "gpt-new",
                "gpt-old",
                "claude-new",
                "claude-old",
                "gemini-new",
                "gemini-old",
                "gemini-undated",
                "unknown",
            ]
        );
        assert_eq!(catalog.merge_display_order(&reset, &[]), reset);
    }

    #[test]
    fn saved_models_keep_their_relative_order() {
        let catalog = catalog(
            r#"{
                "openai/new":{"name":"New","family":"gpt","release_date":"2026-01-01"},
                "openai/middle":{"name":"Middle","family":"gpt","release_date":"2025-01-01"},
                "openai/old":{"name":"Old","family":"gpt","release_date":"2024-01-01"}
            }"#,
        );
        let ordered = catalog.merge_display_order(
            ["old", "middle", "new"],
            &["old".to_string(), "new".to_string()],
        );
        assert_eq!(ordered, ["middle", "old", "new"]);
        assert_eq!(
            ordered
                .iter()
                .filter(|model| ["old", "new"].contains(&model.as_str()))
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["old", "new"]
        );
    }

    #[test]
    fn cache_hash_rejects_modified_payload() {
        let payload = serde_json::json!({"openai/test":{"name":"Test","family":"gpt"}});
        let mut envelope = MetadataCacheEnvelope::new(payload, 1).unwrap();
        envelope.payload = serde_json::json!({"openai/other":{"name":"Other","family":"gpt"}});
        assert_eq!(envelope.validate(), Err(ModelMetadataError::InvalidCache));
    }
}

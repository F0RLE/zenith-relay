use super::{
    order, parsing, reference, ModelCapabilities, ModelMetadata, ModelMetadataCatalog,
    ModelMetadataError,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

impl ModelMetadataCatalog {
    /// Presentation only: resolving a label must never rewrite the route ID or
    /// promote catalog metadata into an account's native transport contract.
    pub fn codex_display_name(&self, model: &str) -> String {
        self.resolve(model)
            .and_then(|metadata| metadata.name.as_deref())
            .map(str::trim)
            .filter(|display_name| {
                !display_name.is_empty() && !display_name.chars().any(char::is_control)
            })
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

    pub fn apply_codex_capabilities(&self, model: &str, catalog_entry: &mut Value) {
        self.capabilities_for(model).apply_to_codex(catalog_entry);
        crate::catalog::set_codex_service_tiers(catalog_entry, &self.service_tiers_for(model));
    }

    pub fn service_tiers_for(&self, model: &str) -> Vec<crate::DefaultServiceTier> {
        crate::catalog::model_service_tiers(
            model,
            self.resolve(model)
                .map(|metadata| metadata.capabilities.service_tiers.as_slice())
                .unwrap_or_default(),
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
            alias_matches: BTreeMap::new(),
            family_providers: BTreeMap::new(),
        }
    }

    pub fn from_models_dev_json(models_dev_json: &str) -> Result<Self, ModelMetadataError> {
        let catalog_payload = serde_json::from_str(models_dev_json)
            .map_err(|_| ModelMetadataError::InvalidCatalog)?;
        Self::from_metadata_payload(&catalog_payload, None, None, false)
    }

    pub(super) fn from_metadata_payload(
        metadata_payload: &Value,
        revision: Option<String>,
        fetched_at_ms: Option<u64>,
        stale: bool,
    ) -> Result<Self, ModelMetadataError> {
        let metadata_records = parsing::validate_metadata_payload(metadata_payload)?;
        let mut metadata_entries = BTreeMap::new();
        let mut leaf_matches = BTreeMap::new();
        let mut ambiguous_leaves = BTreeSet::new();
        let mut leaf_priorities = BTreeMap::new();
        let mut family_providers = BTreeMap::<String, Option<String>>::new();

        for (source_id, metadata_record) in metadata_records {
            let provider_namespace = source_id
                .split_once('/')
                .map_or("", |(provider, _)| provider);
            let Some(mut metadata) =
                parsing::parse_metadata(&source_id, provider_namespace, metadata_record)
            else {
                continue;
            };
            super::protocol_policy::apply(&mut metadata);
            let normalized_model_id = order::normalize(&source_id);
            let leaf = order::model_leaf(&normalized_model_id).to_string();
            let priority = reference::identity_priority(metadata_record);
            // Only the primary identity catalog can establish the owner of a
            // family. Hosting records may supply a canonical ID for their own
            // model, but must not redefine a whole family's manufacturer.
            if priority == 0 {
                if let (Some(family), Some(provider)) = (
                    metadata.family.as_deref().map(order::normalize),
                    metadata
                        .canonical_model_id
                        .as_deref()
                        .and_then(order::canonical_namespace)
                        .or_else(|| order::provider_key(&metadata.provider)),
                ) {
                    family_providers
                        .entry(family)
                        .and_modify(|existing| {
                            if existing.as_ref() != Some(&provider) {
                                *existing = None;
                            }
                        })
                        .or_insert(Some(provider));
                }
            }
            metadata_entries.insert(normalized_model_id.clone(), metadata);

            // Supplemental hosting catalogs must not make a canonical model
            // lose its unqualified identity. Equal-priority conflicts remain
            // ambiguous unless their semantic metadata is equivalent; exact
            // qualified IDs always resolve independently.
            match leaf_priorities.get(&leaf) {
                Some(previous_priority) if *previous_priority < priority => continue,
                Some(previous_priority) if *previous_priority == priority => {}
                _ => {
                    leaf_priorities.insert(leaf.clone(), priority);
                    ambiguous_leaves.remove(&leaf);
                    leaf_matches.insert(leaf, normalized_model_id);
                    continue;
                }
            }
            if ambiguous_leaves.contains(&leaf) {
                continue;
            }
            if let Some(previous_model_id) =
                leaf_matches.insert(leaf.clone(), normalized_model_id.clone())
            {
                if previous_model_id != normalized_model_id {
                    // Share metadata only when descriptive identity and every
                    // semantic field agree. The reference source ID is kept
                    // for identity/provenance, but is not a semantic
                    // capability field. Exact provider IDs remain separate;
                    // this never merges routes or participant inventories.
                    let current_metadata = &metadata_entries[&normalized_model_id];
                    if current_metadata.name.is_some()
                        && (current_metadata.family.is_some()
                            || current_metadata.release_date.is_some())
                        && equivalent_reference_metadata(
                            &metadata_entries[&previous_model_id],
                            current_metadata,
                        )
                    {
                        leaf_matches.insert(leaf, previous_model_id);
                    } else {
                        leaf_matches.remove(&leaf);
                        ambiguous_leaves.insert(leaf);
                    }
                }
            }
        }

        if metadata_entries.is_empty() {
            return Err(ModelMetadataError::InvalidCatalog);
        }

        let mut alias_matches = BTreeMap::<String, Option<String>>::new();
        for (id, metadata) in &metadata_entries {
            for alias in &metadata.aliases {
                let alias = order::normalize(alias);
                for key in [alias.clone(), order::model_leaf(&alias).to_string()] {
                    alias_matches
                        .entry(key)
                        .and_modify(|owner| {
                            if owner.as_ref() != Some(id) {
                                *owner = None;
                            }
                        })
                        .or_insert_with(|| Some(id.clone()));
                }
            }
        }

        Ok(Self {
            revision,
            fetched_at_ms,
            stale,
            sources: BTreeMap::new(),
            entries: metadata_entries,
            leaf_matches,
            ambiguous_leaves,
            alias_matches,
            family_providers,
        })
    }

    /// Resolve exact catalog IDs first. An unqualified or Relay-qualified ID
    /// may use its leaf only when that leaf identifies one catalog record.
    pub fn resolve(&self, model: &str) -> Option<&ModelMetadata> {
        let normalized_model_id = order::normalize(model);
        if let Some(metadata) = self.entries.get(&normalized_model_id) {
            return Some(metadata);
        }
        if let Some(owner) = self.alias_matches.get(&normalized_model_id) {
            if normalized_model_id.contains('/') {
                return owner.as_ref().and_then(|id| self.entries.get(id));
            }
        }
        let leaf = order::model_leaf(&normalized_model_id);
        if self.ambiguous_leaves.contains(leaf) {
            return None;
        }
        self.leaf_matches
            .get(leaf)
            .and_then(|model_id| self.entries.get(model_id))
            .or_else(|| {
                self.alias_matches
                    .get(leaf)
                    .and_then(Option::as_ref)
                    .and_then(|id| self.entries.get(id))
            })
    }

    /// Presentation only: prefer the canonical manufacturer, then a family
    /// with one owner in the primary catalog or an unambiguous leaf reference.
    /// A record without either keeps its reference namespace.
    /// Name-based fallback is limited to unresolved familiar model IDs. It
    /// must never choose a protocol, grant a route, or merge model identities.
    pub fn display_provider_for(&self, model: &str) -> Option<String> {
        let Some(metadata) = self.resolve(model) else {
            let normalized = order::normalize(model);
            if self
                .ambiguous_leaves
                .contains(order::model_leaf(&normalized))
            {
                return None;
            }
            return order::inferred_provider(model);
        };
        metadata
            .canonical_model_id
            .as_deref()
            .and_then(order::canonical_namespace)
            .or_else(|| {
                metadata.family.as_deref().and_then(|family| {
                    self.family_providers
                        .get(&order::normalize(family))
                        .cloned()
                        .flatten()
                })
            })
            .or_else(|| {
                let normalized_id = order::normalize(&metadata.source_model_id);
                let reference_id = self.leaf_matches.get(order::model_leaf(&normalized_id))?;
                let reference = self.entries.get(reference_id)?;
                reference
                    .canonical_model_id
                    .as_deref()
                    .and_then(order::canonical_namespace)
                    .or_else(|| order::provider_key(&reference.provider))
            })
            .or_else(|| order::provider_key(&metadata.provider))
    }

    /// Prefer the manufacturer's native API for known model groups.
    /// Exact source endpoint declarations take
    /// precedence; this preference does not establish endpoint support.
    ///
    /// The group comes from the validated reference record: the canonical
    /// model ID's namespace first, then a first-party namespace, then the
    /// family. A hosting namespace alone never decides it. Unknown or
    /// ambiguous models return `None` so callers keep their other evidence.
    pub fn preferred_protocol_for(&self, model: &str) -> Option<crate::WireApi> {
        self.protocol_preferences_for(model).first().copied()
    }

    /// Select the provider contract before adapting the client's protocol.
    /// Only validated manufacturer/family identity supplies this requirement;
    /// an arbitrary model name or reseller namespace cannot establish it.
    pub fn native_protocol_for(&self, model: &str) -> Option<crate::WireApi> {
        self.resolve(model)
            .and_then(super::protocol_policy::native_protocol)
    }

    pub fn protocol_preferences_for(&self, model: &str) -> Vec<crate::WireApi> {
        self.resolve(model)
            .map(|metadata| metadata.protocol_preferences.clone())
            .unwrap_or_default()
    }

    pub(super) fn bind_field_revisions(&mut self) {
        for metadata in self.entries.values_mut() {
            for origin in metadata.field_provenance.values_mut() {
                if let Some(status) = self.sources.get(&origin.source) {
                    origin.revision = status.revision.clone();
                }
                for conflict in &mut origin.conflicts {
                    if let Some(status) = self.sources.get(&conflict.source) {
                        conflict.revision = status.revision.clone();
                    }
                }
            }
        }
    }

    pub fn reasoning_effort_levels(&self, model: &str) -> Option<Vec<String>> {
        self.resolve(model)
            .map(|metadata| metadata.capabilities.reasoning_effort_levels.clone())
    }

    pub fn display_identity_for(&self, model: &str) -> super::ModelDisplayIdentity {
        self.display_order_key(model).identity
    }

    fn display_order_key(&self, model: &str) -> order::ModelOrderKey {
        let metadata = self.resolve(model);
        order::ModelOrderKey::new(
            metadata
                .and_then(|metadata| metadata.canonical_model_id.as_deref())
                .unwrap_or(model),
            self.display_provider_for(model),
            metadata.and_then(|metadata| metadata.family.as_deref()),
            metadata.and_then(|metadata| metadata.name.as_deref()),
            metadata.and_then(|metadata| metadata.release_date.as_deref()),
        )
    }

    /// Apply Relay's shared provider/family/class policy and descending
    /// numeric versions. The original route IDs and inventory stay intact.
    pub fn order_model_ids<I, S>(&self, models: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut models = crate::normalize_model_ids(models)
            .into_iter()
            .map(|model| {
                let key = self.display_order_key(&model);
                (model, key)
            })
            .collect::<Vec<_>>();
        models.sort_by(|(left, left_key), (right, right_key)| {
            left_key
                .compare(right_key)
                .then_with(|| crate::model_id_key(left).cmp(&crate::model_id_key(right)))
        });
        models.into_iter().map(|(model, _)| model).collect()
    }

    /// Preserve the relative order explicitly saved by the user. Newly
    /// discovered models are inserted at their provider-block position around
    /// those anchors instead of resetting the complete list.
    pub fn merge_display_order<I, S>(&self, models: I, saved_order: &[String]) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let models = self.order_model_ids(models);
        if saved_order.is_empty() {
            return models;
        }
        let groups = models
            .iter()
            .map(|model| {
                (
                    crate::model_id_key(model),
                    self.display_provider_for(model)
                        .unwrap_or_else(|| "other".to_string()),
                )
            })
            .collect();
        crate::catalog::merge_model_display_order_with_groups(models, saved_order, &groups)
    }
}

/// Compare catalog semantics while keeping source identity and provenance
/// independent from the metadata used to resolve an unqualified display name.
/// Canonical identity remains part of the comparison: two aliases are
/// equivalent only when they point at the same canonical model (or both omit
/// that relation).
fn equivalent_reference_metadata(left: &ModelMetadata, right: &ModelMetadata) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.source_model_id.clear();
    right.source_model_id.clear();
    left.provider = right.provider.clone();
    left.field_provenance.clear();
    right.field_provenance.clear();
    left.aliases.clear();
    right.aliases.clear();
    left == right
}

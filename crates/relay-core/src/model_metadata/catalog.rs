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
        let catalog_payload =
            serde_json::from_str(raw).map_err(|_| ModelMetadataError::InvalidCatalog)?;
        Self::from_payload(&catalog_payload, None, None, false)
    }

    pub(super) fn from_payload(
        payload: &Value,
        revision: Option<String>,
        fetched_at_ms: Option<u64>,
        stale: bool,
    ) -> Result<Self, ModelMetadataError> {
        let records = parsing::validate_payload(payload)?;
        let mut metadata_entries = BTreeMap::new();
        let mut leaf_matches = BTreeMap::new();
        let mut ambiguous_leaves = BTreeSet::new();
        let mut leaf_priorities = BTreeMap::new();

        for (source_id, metadata_record) in records {
            let provider_namespace = source_id
                .split_once('/')
                .map_or("", |(provider, _)| provider);
            let Some(metadata) =
                parsing::parse_metadata(&source_id, provider_namespace, metadata_record)
            else {
                continue;
            };
            let normalized_model_id = order::normalize(&source_id);
            let leaf = order::model_leaf(&normalized_model_id).to_string();
            metadata_entries.insert(normalized_model_id.clone(), metadata);

            // Supplemental hosting catalogs must not make a canonical model
            // lose its unqualified identity. Equal-priority conflicts remain
            // ambiguous unless their semantic metadata is equivalent; exact
            // qualified IDs always resolve independently.
            let priority = reference::identity_priority(metadata_record);
            match leaf_priorities.get(&leaf) {
                Some(previous) if *previous < priority => continue,
                Some(previous) if *previous == priority => {}
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

        Ok(Self {
            revision,
            fetched_at_ms,
            stale,
            sources: BTreeMap::new(),
            entries: metadata_entries,
            leaf_matches,
            ambiguous_leaves,
        })
    }

    /// Resolve exact catalog IDs first. An unqualified or Relay-qualified ID
    /// may use its leaf only when that leaf identifies one catalog record.
    pub fn resolve(&self, model: &str) -> Option<&ModelMetadata> {
        let normalized_model_id = order::normalize(model);
        if let Some(metadata) = self.entries.get(&normalized_model_id) {
            return Some(metadata);
        }
        let leaf = order::model_leaf(&normalized_model_id);
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

    /// Keep provider blocks together while preserving the order supplied by
    /// the account or API source inside each block. Presentation never
    /// determines eligibility and the catalog must not invent a model ranking
    /// from release dates, family names, or model IDs.
    pub fn order_model_ids<I, S>(&self, models: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let source_model_ids = crate::normalize_model_ids(models);
        let mut indexed = source_model_ids.into_iter().enumerate().collect::<Vec<_>>();
        indexed.sort_by(|(left_position, left_id), (right_position, right_id)| {
            order::compare_metadata(
                left_id,
                self.resolve(left_id),
                right_id,
                self.resolve(right_id),
            )
            .then_with(|| left_position.cmp(right_position))
        });
        indexed.into_iter().map(|(_, id)| id).collect()
    }

    /// Preserve the relative order explicitly saved by the user. Newly
    /// discovered models are inserted at their provider-block position around
    /// those anchors instead of resetting the complete list.
    pub fn merge_display_order<I, S>(&self, models: I, saved_order: &[String]) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let source_model_ids = crate::normalize_model_ids(models);
        let available_model_ids = source_model_ids
            .iter()
            .map(|id| order::normalize(id))
            .collect::<BTreeSet<_>>();
        let catalog_order = self.order_model_ids(source_model_ids);
        if !saved_order
            .iter()
            .any(|id| available_model_ids.contains(&order::normalize(id)))
        {
            return catalog_order;
        }

        let catalog_positions = catalog_order
            .iter()
            .enumerate()
            .map(|(position, id)| (order::normalize(id), position))
            .collect::<BTreeMap<_, _>>();
        let mut saved_model_ids = BTreeSet::new();
        let mut ordered_model_ids = Vec::with_capacity(catalog_order.len());

        for id in saved_order {
            let normalized_id = order::normalize(id);
            if saved_model_ids.insert(normalized_id.clone()) {
                if let Some(position) = catalog_positions.get(&normalized_id) {
                    ordered_model_ids.push(catalog_order[*position].clone());
                }
            }
        }
        for id in catalog_order {
            let normalized_id = order::normalize(&id);
            if saved_model_ids.contains(&normalized_id) {
                continue;
            }
            let position = catalog_positions[&normalized_id];
            let insertion_index = ordered_model_ids
                .iter()
                .position(|existing| catalog_positions[&order::normalize(existing)] > position)
                .unwrap_or(ordered_model_ids.len());
            ordered_model_ids.insert(insertion_index, id);
        }
        ordered_model_ids
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
    left == right
}

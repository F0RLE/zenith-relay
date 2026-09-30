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
        let payload = serde_json::from_str(raw).map_err(|_| ModelMetadataError::InvalidCatalog)?;
        Self::from_payload(&payload, None, None, false)
    }

    pub(super) fn from_payload(
        payload: &Value,
        revision: Option<String>,
        fetched_at_ms: Option<u64>,
        stale: bool,
    ) -> Result<Self, ModelMetadataError> {
        let records = parsing::validate_payload(payload)?;
        let mut entries = BTreeMap::new();
        let mut leaf_matches = BTreeMap::new();
        let mut ambiguous_leaves = BTreeSet::new();
        let mut leaf_priorities = BTreeMap::new();

        for (source_id, value) in records {
            let provider = source_id
                .split_once('/')
                .map_or("", |(provider, _)| provider);
            let Some(metadata) = parsing::parse_metadata(provider, value) else {
                continue;
            };
            let key = order::normalize(&source_id);
            let leaf = order::model_leaf(&key).to_string();
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
        let key = order::normalize(model);
        if let Some(metadata) = self.entries.get(&key) {
            return Some(metadata);
        }
        let leaf = order::model_leaf(&key);
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

    /// Keep companies and catalog families together. Provider family precedence
    /// is applied where the catalog has a stable product-tier order; release /
    /// update dates then order versions inside each family. Presentation never
    /// determines eligibility.
    pub fn order_model_ids<I, S>(&self, models: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let source = crate::normalize_model_ids(models);
        let family_order = order::family_order(self, &source);
        let mut indexed = source.into_iter().collect::<Vec<_>>();
        indexed.sort_by(|left_id, right_id| {
            order::compare_metadata(
                left_id,
                self.resolve(left_id),
                right_id,
                self.resolve(right_id),
                &family_order,
            )
            .then_with(|| order::normalize(left_id).cmp(&order::normalize(right_id)))
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
            .map(|id| order::normalize(id))
            .collect::<BTreeSet<_>>();
        let catalog_order = self.order_model_ids(source);
        if !saved_order
            .iter()
            .any(|id| available.contains(&order::normalize(id)))
        {
            return catalog_order;
        }

        let positions = catalog_order
            .iter()
            .enumerate()
            .map(|(position, id)| (order::normalize(id), position))
            .collect::<BTreeMap<_, _>>();
        let mut saved = BTreeSet::new();
        let mut ordered = Vec::with_capacity(catalog_order.len());

        for id in saved_order {
            let key = order::normalize(id);
            if saved.insert(key.clone()) {
                if let Some(position) = positions.get(&key) {
                    ordered.push(catalog_order[*position].clone());
                }
            }
        }
        for id in catalog_order {
            let key = order::normalize(&id);
            if saved.contains(&key) {
                continue;
            }
            let position = positions[&key];
            let insertion = ordered
                .iter()
                .position(|existing| positions[&order::normalize(existing)] > position)
                .unwrap_or(ordered.len());
            ordered.insert(insertion, id);
        }
        ordered
    }
}

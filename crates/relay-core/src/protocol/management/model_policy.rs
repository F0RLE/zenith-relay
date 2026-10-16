use crate::{
    error_codes, is_valid_model_id, model_id_key, normalize_model_reasoning_allowed_levels,
    reasoning_policy_key,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelPolicyError {
    InvalidId,
    NotFound,
    DuplicateOrderEntry,
}

impl ModelPolicyError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidId => error_codes::MODEL_ID_INVALID,
            Self::NotFound => error_codes::MODEL_NOT_FOUND,
            Self::DuplicateOrderEntry => error_codes::MODEL_ORDER_INVALID,
        }
    }
}

impl fmt::Display for ModelPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "model id is invalid",
            Self::NotFound => "pool model not found",
            Self::DuplicateOrderEntry => "model order contains duplicates",
        })
    }
}

impl std::error::Error for ModelPolicyError {}

/// Resolve an editable model against configured inventory, independently of
/// runtime health or protocol eligibility. The first matching ID owns casing;
/// callers provide their existing inventory precedence without cloning it.
pub fn canonical_pool_model_id<'a>(
    model_ids: impl IntoIterator<Item = &'a String>,
    requested: &str,
) -> Result<&'a str, ModelPolicyError> {
    let requested = requested.trim();
    if !is_valid_model_id(requested) {
        return Err(ModelPolicyError::InvalidId);
    }
    model_ids
        .into_iter()
        .find(|model_id| model_id_key(model_id) == model_id_key(requested))
        .map(String::as_str)
        .ok_or(ModelPolicyError::NotFound)
}

/// Configured source inventory, including model ids that exist only on a
/// protocol binding. Runtime health does not filter this list.
pub fn configured_source_model_ids<'a>(
    models: &'a [String],
    bindings: &'a [crate::SourceProtocolBinding],
) -> impl Iterator<Item = &'a String> + 'a {
    models
        .iter()
        .chain(bindings.iter().flat_map(|binding| binding.model_ids.iter()))
}

/// Complete a partial display order without losing inventory absent from a
/// stale client. Empty input resets the override; stale saved IDs are ignored.
/// New IDs follow the configured inventory sequence, preserving canonical
/// casing instead of introducing a second alphabetical order.
pub fn complete_model_display_order<'a>(
    model_ids: impl IntoIterator<Item = &'a String>,
    requested_ids: &[String],
    saved_order: &[String],
) -> Result<Vec<String>, ModelPolicyError> {
    if requested_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut remaining = BTreeMap::new();
    let mut inventory_order = Vec::new();
    for model_id in model_ids {
        let key = model_id_key(model_id);
        if let std::collections::btree_map::Entry::Vacant(entry) = remaining.entry(key.clone()) {
            entry.insert(model_id);
            inventory_order.push(key);
        }
    }

    let mut order = Vec::with_capacity(remaining.len());
    let mut requested = BTreeSet::new();
    for requested_model_id in requested_ids {
        let key = model_id_key(requested_model_id);
        if !requested.insert(key.clone()) {
            return Err(ModelPolicyError::DuplicateOrderEntry);
        }
        let canonical = remaining.remove(&key).ok_or(ModelPolicyError::NotFound)?;
        order.push(canonical.clone());
    }
    for saved_model_id in saved_order {
        if let Some(canonical) = remaining.remove(&model_id_key(saved_model_id)) {
            order.push(canonical.clone());
        }
    }
    for key in inventory_order {
        if let Some(canonical) = remaining.remove(&key) {
            order.push(canonical.clone());
        }
    }
    Ok(order)
}

/// Apply one reasoning edit after validating it. The shared family policy
/// replaces this model's legacy override; unrelated settings remain intact.
/// An empty list is retained as an explicit choice to disable every level.
pub fn update_model_reasoning_policy(
    policies: &mut BTreeMap<String, Vec<String>>,
    model: &str,
    requested_levels: Vec<String>,
) -> Result<(), &'static str> {
    let normalized = normalize_model_reasoning_allowed_levels(BTreeMap::from([(
        reasoning_policy_key(model),
        requested_levels,
    )]))?;
    policies.remove(&model_id_key(model));
    policies.extend(normalized);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_ids(model_names: &[&str]) -> Vec<String> {
        model_names
            .iter()
            .map(|model_name| (*model_name).to_string())
            .collect()
    }

    #[test]
    fn canonical_id_preserves_first_configured_spelling_and_qualified_identity() {
        let inventory = model_ids(&["Model-A", "model-a", "provider/Model-A"]);
        assert_eq!(
            canonical_pool_model_id(&inventory, " MODEL-A "),
            Ok("Model-A")
        );
        assert_eq!(
            canonical_pool_model_id(&inventory, "PROVIDER/model-a"),
            Ok("provider/Model-A")
        );
        assert_eq!(
            canonical_pool_model_id(&inventory, "other/Model-A"),
            Err(ModelPolicyError::NotFound)
        );
    }

    #[test]
    fn canonical_id_distinguishes_invalid_input_from_missing_inventory() {
        let inventory = model_ids(&["model-a"]);
        for invalid in ["", "  ", "model\nname", "model\0name"] {
            assert_eq!(
                canonical_pool_model_id(&inventory, invalid),
                Err(ModelPolicyError::InvalidId)
            );
        }
        assert_eq!(
            canonical_pool_model_id(&inventory, "missing"),
            Err(ModelPolicyError::NotFound)
        );
        assert_eq!(
            canonical_pool_model_id(&inventory, &"x".repeat(257)),
            Err(ModelPolicyError::InvalidId)
        );
        assert_eq!(
            canonical_pool_model_id(&[], "model-a"),
            Err(ModelPolicyError::NotFound)
        );
    }

    #[test]
    fn empty_order_resets_override_even_with_stale_saved_inventory() {
        let saved = model_ids(&["missing", "model-a"]);
        for inventory in [vec![], model_ids(&["model-a"])] {
            assert_eq!(
                complete_model_display_order(&inventory, &[], &saved),
                Ok(vec![])
            );
        }
    }

    #[test]
    fn partial_order_preserves_omitted_models_and_appends_new_inventory() {
        let inventory = model_ids(&["new-z", "model-a", "new-c", "model-b", "hidden"]);
        let requested = model_ids(&[" MODEL-B ", "model-a"]);
        let saved = model_ids(&["stale", "hidden", "HIDDEN", "model-a"]);
        assert_eq!(
            complete_model_display_order(&inventory, &requested, &saved),
            Ok(model_ids(&[
                "model-b", "model-a", "hidden", "new-z", "new-c"
            ]))
        );
    }

    #[test]
    fn repeated_member_models_share_one_position_without_merging_qualified_ids() {
        let inventory = model_ids(&["Model-A", "model-a", "provider/model-a", "Model-B"]);
        let requested = model_ids(&["model-b"]);
        let saved = model_ids(&["MODEL-A", "provider/model-a"]);
        assert_eq!(
            complete_model_display_order(&inventory, &requested, &saved),
            Ok(model_ids(&["Model-B", "Model-A", "provider/model-a"]))
        );
    }

    #[test]
    fn unknown_or_duplicate_requested_ids_are_rejected() {
        let inventory = model_ids(&["model-a", "model-b"]);
        for requested in [model_ids(&["missing"]), model_ids(&["model-b", "missing"])] {
            assert_eq!(
                complete_model_display_order(&inventory, &requested, &[]),
                Err(ModelPolicyError::NotFound)
            );
        }
        assert_eq!(
            complete_model_display_order(&inventory, &model_ids(&["model-a", " MODEL-A "]), &[]),
            Err(ModelPolicyError::DuplicateOrderEntry)
        );
    }

    #[test]
    fn reasoning_edit_replaces_only_the_selected_models_legacy_override() {
        let mut policies = BTreeMap::from([
            ("gpt-test".into(), model_ids(&["medium"])),
            ("gpt-other".into(), model_ids(&["low"])),
            ("group:openai".into(), model_ids(&["minimal"])),
            ("group:anthropic".into(), model_ids(&["high"])),
        ]);
        update_model_reasoning_policy(
            &mut policies,
            "GPT-Test",
            model_ids(&[" HIGH ", "low", "HIGH"]),
        )
        .unwrap();
        assert_eq!(
            policies,
            BTreeMap::from([
                ("gpt-other".into(), model_ids(&["low"])),
                ("group:openai".into(), model_ids(&["low", "high"])),
                ("group:anthropic".into(), model_ids(&["high"])),
            ])
        );
    }

    #[test]
    fn empty_reasoning_edit_remains_an_explicit_override_for_known_and_unknown_families() {
        for (model, key) in [
            ("gpt-test", "group:openai"),
            ("Custom-Model", "custom-model"),
        ] {
            let mut policies = BTreeMap::from([(key.to_string(), model_ids(&["high"]))]);
            update_model_reasoning_policy(&mut policies, model, vec![]).unwrap();
            assert_eq!(policies.get(key), Some(&Vec::new()));
        }
    }

    #[test]
    fn invalid_reasoning_edit_does_not_partially_change_the_policy() {
        let previous_policies = BTreeMap::from([
            ("gpt-test".into(), model_ids(&["low"])),
            ("group:openai".into(), model_ids(&["high"])),
        ]);
        for levels in [
            model_ids(&[" "]),
            model_ids(&["high", "invalid\nlevel"]),
            vec!["low".into(); 65],
        ] {
            let mut policies = previous_policies.clone();
            assert!(update_model_reasoning_policy(&mut policies, "gpt-test", levels).is_err());
            assert_eq!(policies, previous_policies);
        }
    }
}

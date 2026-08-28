use super::GatewayRuntime;
use serde_json::Value;

impl GatewayRuntime {
    /// Fast is an upstream entitlement, not a model-name heuristic. The
    /// management surface consults the last confirmed manifest for an eligible
    /// route, so a free or otherwise ineligible account never gets a fabricated
    /// Fast toggle.
    pub fn model_supports_fast_service_tier(&self, model: &str) -> bool {
        let model = model.trim();
        if model.is_empty() {
            return false;
        }
        self.current_candidate_ids_for_model(model)
            .into_iter()
            .any(|candidate_id| self.candidate_supports_fast_service_tier(&candidate_id, model))
    }

    /// Returns Fast capability for one concrete scheduler route. A model can
    /// be shared by several providers, so a manifest from one route must never
    /// authorize a priority request on another route.
    pub(crate) fn candidate_supports_fast_service_tier(
        &self,
        candidate_id: &str,
        model: &str,
    ) -> bool {
        let model = model.trim();
        if candidate_id.trim().is_empty() || model.is_empty() {
            return false;
        }
        let has_model = self
            .lock_scheduler()
            .candidate(candidate_id)
            .is_some_and(|candidate| {
                candidate
                    .models
                    .iter()
                    .any(|candidate_model| candidate_model.eq_ignore_ascii_case(model))
            });
        if !has_model {
            return false;
        }
        let manifest = if self.source_candidate_bindings.contains_key(candidate_id) {
            self.model_metadata
                .source_manifests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(candidate_id)
                .cloned()
        } else if self.chatgpt_accounts.contains_key(candidate_id) {
            self.model_metadata
                .codex_manifests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(candidate_id)
                .cloned()
        } else {
            None
        };
        manifest.is_some_and(|manifest| manifest_supports_fast_service_tier(&manifest.value, model))
    }

    fn current_candidate_ids_for_model(&self, model: &str) -> Vec<String> {
        let scheduler = self.lock_scheduler();
        scheduler
            .candidates()
            .filter(|candidate| {
                candidate
                    .models
                    .iter()
                    .any(|candidate_model| candidate_model.eq_ignore_ascii_case(model))
            })
            .map(|candidate| candidate.id.clone())
            .collect()
    }
}

fn manifest_supports_fast_service_tier(manifest: &Value, model: &str) -> bool {
    let Some(models) = manifest
        .get("models")
        .or_else(|| manifest.get("data"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    models.iter().any(|entry| {
        let Some(object) = entry.as_object() else {
            return false;
        };
        let id = object
            .get("slug")
            .or_else(|| object.get("id"))
            .and_then(Value::as_str)
            .map(str::trim);
        if !id.is_some_and(|id| id.eq_ignore_ascii_case(model)) {
            return false;
        }
        object
            .get("service_tiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers.iter().any(|tier| {
                    tier.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(is_fast_service_tier)
                })
            })
            || object
                .get("additional_speed_tiers")
                .and_then(Value::as_array)
                .is_some_and(|tiers| {
                    tiers
                        .iter()
                        .filter_map(Value::as_str)
                        .any(is_fast_service_tier)
                })
            || object
                .get("default_service_tier")
                .and_then(Value::as_str)
                .is_some_and(is_fast_service_tier)
    })
}

fn is_fast_service_tier(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "fast" | "priority"
    )
}

use super::{CachedModelManifest, GatewayRuntime};
use serde_json::Value;

impl GatewayRuntime {
    pub(crate) fn set_codex_model_uses_responses_lite(
        &self,
        candidate_id: &str,
        model: &str,
        enabled: bool,
    ) {
        let mut models = self
            .codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (candidate_id.to_string(), model.to_ascii_lowercase());
        if enabled {
            models.insert(key);
        } else {
            models.remove(&key);
        }
    }

    pub(crate) fn codex_model_responses_lite_candidates(&self, model: &str) -> Vec<String> {
        let model = model.to_ascii_lowercase();
        self.codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, candidate_model)| candidate_model == &model)
            .map(|(candidate_id, _)| candidate_id.clone())
            .collect()
    }

    pub(crate) fn remember_codex_model_manifest(
        &self,
        candidate_id: &str,
        value: Value,
        observed_at_ms: u64,
    ) {
        let scheduler = self.lock_scheduler();
        if scheduler.candidate(candidate_id).is_none() {
            return;
        }
        self.model_metadata
            .codex_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                candidate_id.to_string(),
                CachedModelManifest {
                    value,
                    observed_at_ms,
                },
            );
    }

    pub(crate) fn stale_codex_model_manifests<'a>(
        &self,
        candidate_ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<(String, Value)> {
        let manifests = self
            .model_metadata
            .codex_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        candidate_ids
            .into_iter()
            .filter_map(|candidate_id| {
                manifests
                    .get(candidate_id)
                    .map(|manifest| (candidate_id.to_string(), manifest.value.clone()))
            })
            .collect()
    }
}

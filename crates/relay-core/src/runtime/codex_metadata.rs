use super::{CachedModelManifest, GatewayRuntime};
use serde_json::Value;
use std::collections::BTreeMap;

impl GatewayRuntime {
    pub fn set_official_codex_ultra_models(&self, models: BTreeMap<String, Value>) {
        *crate::poison::mutex(&self.official_codex_ultra) = models;
    }

    pub(crate) fn official_codex_ultra_model(&self, model: &str) -> Option<Value> {
        crate::poison::mutex(&self.official_codex_ultra)
            .get(&crate::model_id_key(model))
            .cloned()
    }

    pub(crate) fn set_codex_model_uses_responses_lite(
        &self,
        candidate_id: &str,
        model: &str,
        enabled: bool,
    ) {
        let mut models = crate::poison::mutex(&self.codex_responses_lite_models);
        let key = (candidate_id.to_string(), crate::model_id_key(model));
        if enabled {
            models.insert(key);
        } else {
            models.remove(&key);
        }
    }

    pub(crate) fn codex_model_responses_lite_candidates(&self, model: &str) -> Vec<String> {
        let model = crate::model_id_key(model);
        crate::poison::mutex(&self.codex_responses_lite_models)
            .iter()
            .filter(|(_, candidate_model)| candidate_model == &model)
            .map(|(candidate_id, _)| candidate_id.clone())
            .collect()
    }

    pub(crate) fn remember_codex_model_manifest(
        &self,
        candidate_id: &str,
        value: Value,
        _observed_at_ms: u64,
    ) {
        let scheduler = self.lock_scheduler();
        if scheduler.candidate(candidate_id).is_none() {
            return;
        }
        crate::poison::mutex(&self.model_metadata.codex_manifests)
            .insert(candidate_id.to_string(), CachedModelManifest { value });
    }

    pub(crate) fn stale_codex_model_manifests<'a>(
        &self,
        candidate_ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<(String, Value)> {
        let manifests = crate::poison::mutex(&self.model_metadata.codex_manifests);
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

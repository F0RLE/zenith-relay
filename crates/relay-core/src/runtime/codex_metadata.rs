use super::{CachedModelManifest, GatewayRuntime};
use serde_json::Value;
use std::collections::BTreeMap;

impl GatewayRuntime {
    pub(crate) fn model_context_windows<'a>(
        &self,
        model: &str,
        native_candidate_ids: impl IntoIterator<Item = &'a str>,
    ) -> crate::catalog::context_policy::ModelContextWindows {
        use crate::catalog::context_policy::ModelContextWindows;

        let candidate_ids = native_candidate_ids.into_iter().collect::<Vec<_>>();
        if candidate_ids.is_empty() {
            return ModelContextWindows {
                default_tokens: None,
                maximum_tokens: self
                    .model_capabilities(model)
                    .context_limit
                    .filter(|tokens| *tokens > 0),
            };
        }
        let model_id = crate::model_id_key(model);
        let mut windows = ModelContextWindows::default();
        let manifests = crate::poison::mutex(&self.model_metadata.codex_manifests);
        for candidate_id in candidate_ids {
            let Some(manifest) = manifests.get(candidate_id) else {
                continue;
            };
            for entry in manifest
                .manifest_payload
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if !entry
                    .get("slug")
                    .and_then(Value::as_str)
                    .is_some_and(|slug| crate::model_id_key(slug) == model_id)
                {
                    continue;
                }
                let own = ModelContextWindows::from_entry(entry);
                // Show the conservative confirmed windows across owning
                // accounts. Missing evidence never becomes an API limit.
                windows.default_tokens = windows
                    .default_tokens
                    .into_iter()
                    .chain(own.default_tokens)
                    .min();
                windows.maximum_tokens = windows
                    .maximum_tokens
                    .into_iter()
                    .chain(own.maximum_tokens)
                    .min();
            }
        }
        windows
    }

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
        let model_id = crate::model_id_key(model);
        crate::poison::mutex(&self.codex_responses_lite_models)
            .iter()
            .filter(|(_, candidate_model)| candidate_model == &model_id)
            .map(|(candidate_id, _)| candidate_id.clone())
            .collect()
    }

    pub(crate) fn remember_codex_model_manifest(
        &self,
        candidate_id: &str,
        manifest_payload: Value,
        _observed_at_ms: u64,
    ) {
        let scheduler = self.lock_scheduler();
        if scheduler.candidate(candidate_id).is_none() {
            return;
        }
        crate::poison::mutex(&self.model_metadata.codex_manifests).insert(
            candidate_id.to_string(),
            CachedModelManifest { manifest_payload },
        );
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
                    .map(|manifest| (candidate_id.to_string(), manifest.manifest_payload.clone()))
            })
            .collect()
    }
}

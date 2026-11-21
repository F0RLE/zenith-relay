use super::{AuthorizationIncarnation, CachedModelManifest, GatewayRuntime};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

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
        for (_, manifest) in self.stale_codex_model_manifests(candidate_ids) {
            for entry in manifest
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
        *crate::poison::mutex(&self.provider_metadata.official_codex_ultra) = models;
    }

    pub(crate) fn official_codex_ultra_model(&self, model: &str) -> Option<Value> {
        crate::poison::mutex(&self.provider_metadata.official_codex_ultra)
            .get(&crate::model_id_key(model))
            .cloned()
    }

    pub(crate) fn codex_model_manifest_context(
        &self,
        candidate_id: &str,
    ) -> Option<(Option<String>, u64)> {
        let account = self.account_state.executors.get(candidate_id)?;
        Some((
            crate::poison::read(&account.subscription_plan_type).clone(),
            account.image_bridge_revision.load(Ordering::Acquire),
        ))
    }

    #[cfg(test)]
    pub(crate) fn remember_codex_model_manifest(
        &self,
        candidate_id: &str,
        manifest_payload: Value,
        _observed_at_ms: u64,
    ) {
        if let Some(context) = self.codex_model_manifest_context(candidate_id) {
            self.remember_authorized_codex_model_manifest(
                candidate_id,
                manifest_payload,
                AuthorizationIncarnation::Source,
                context,
            );
        }
    }

    pub(crate) fn remember_authorized_codex_model_manifest(
        &self,
        candidate_id: &str,
        manifest_payload: Value,
        authorization: AuthorizationIncarnation,
        account_context: (Option<String>, u64),
    ) -> bool {
        let Some(account) = self.account_state.executors.get(candidate_id) else {
            return false;
        };
        let plan = crate::poison::read(&account.subscription_plan_type);
        let scheduler = self.lock_scheduler();
        let revision = authorization.token_revision();
        let _credential = revision.and_then(|revision| revision.guard());
        if scheduler.candidate(candidate_id).is_none()
            || !account.active.load(Ordering::Acquire)
            || *plan != account_context.0
            || account.image_bridge_revision.load(Ordering::Acquire) != account_context.1
            || (revision.is_some() && _credential.is_none())
            || matches!(authorization, AuthorizationIncarnation::Agent(expected)
                if account.agent_identity_revision.load(Ordering::Acquire) != expected)
        {
            return false;
        }
        crate::poison::mutex(&self.provider_metadata.codex_manifests).insert(
            candidate_id.to_string(),
            CachedModelManifest {
                manifest_payload,
                authorization: authorization.clone(),
                account_context,
            },
        );
        true
    }

    pub(crate) fn stale_codex_model_manifests<'a>(
        &self,
        candidate_ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<(String, Value)> {
        candidate_ids
            .into_iter()
            .filter_map(|candidate_id| {
                let account = self.account_state.executors.get(candidate_id)?;
                let plan = crate::poison::read(&account.subscription_plan_type);
                let scheduler = self.lock_scheduler();
                if !account.active.load(Ordering::Acquire)
                    || scheduler.candidate(candidate_id).is_none()
                {
                    return None;
                }
                let mut manifests = crate::poison::mutex(&self.provider_metadata.codex_manifests);
                let manifest = manifests.get(candidate_id)?;
                let revision = manifest.authorization.token_revision();
                let credential = revision.and_then(|revision| revision.guard());
                let current = *plan == manifest.account_context.0
                    && account.image_bridge_revision.load(Ordering::Acquire)
                        == manifest.account_context.1
                    && (revision.is_none() || credential.is_some())
                    && !matches!(manifest.authorization, AuthorizationIncarnation::Agent(expected)
                        if account.agent_identity_revision.load(Ordering::Acquire) != expected);
                let result =
                    current.then(|| (candidate_id.to_string(), manifest.manifest_payload.clone()));
                drop(credential);
                if !current {
                    manifests.remove(candidate_id);
                }
                result
            })
            .collect()
    }
}

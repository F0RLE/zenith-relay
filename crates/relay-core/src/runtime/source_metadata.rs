use super::{AuthenticatedKey, GatewayRuntime};
use crate::catalog::{normalize_model_reasoning_allowed_levels, reasoning_policy_levels};
use crate::{Error, Result, WireApi};
use std::collections::{BTreeMap, HashSet};

impl GatewayRuntime {
    pub(crate) fn visible_account_models(&self, key: &AuthenticatedKey) -> Vec<String> {
        let models = self.visible_models(key, &[WireApi::Responses], crate::unix_time_ms());
        let scope = key.scope_snapshot();
        let basis_points_models = self.basis_points_model_snapshot();
        let scheduler = self.lock_scheduler();
        // Filter the shared ordered catalog instead of rebuilding it from
        // account/model BTreeMaps, which would replace catalog order with
        // alphabetical order. Keep the account-only inventory boundary.
        models
            .into_iter()
            .filter(|display_id| {
                let Some(model) = self.resolve_model(key, display_id) else {
                    return false;
                };
                self.chatgpt_accounts.values().any(|account| {
                    let inventory = crate::poison::read(&account.model_inventory);
                    inventory
                        .configured_models
                        .iter()
                        .any(|configured| configured.eq_ignore_ascii_case(&model))
                        && basis_points_models.model_available(&account.id, &model)
                        && scheduler.candidate(&account.id).is_some_and(|candidate| {
                            candidate.is_catalog_visible(&model, &[WireApi::Responses], &scope)
                        })
                })
            })
            .collect()
    }

    pub(crate) fn codex_model_chatgpt_account_ids_for_resolved(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Vec<String> {
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        self.chatgpt_accounts
            .values()
            .filter(|account| {
                crate::poison::read(&account.model_inventory)
                    .configured_models
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(model))
                    && scheduler.candidate(&account.id).is_some_and(|candidate| {
                        candidate.is_configured(model, &[WireApi::Responses], &scope)
                    })
            })
            .map(|account| account.id.clone())
            .collect()
    }

    /// Returns only account routes that speak the native Responses contract.
    /// Excel / Basis Points is an explicit Relay transport and must remain
    /// visible in Relay while avoiding native Codex picker metadata such as
    /// Fast and Ultrafast tiers that its upstream does not confirm.
    pub(crate) fn codex_model_native_responses_account_ids(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Vec<String> {
        let Some(model) = self.resolve_model(key, model) else {
            return Vec::new();
        };
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        self.chatgpt_accounts
            .values()
            .filter(|account| {
                account.oauth_client_kind == crate::providers::chatgpt::OAuthClientKind::Codex
                    && crate::poison::read(&account.model_inventory)
                        .configured_models
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(&model))
                    && scheduler.candidate(&account.id).is_some_and(|candidate| {
                        candidate.is_configured(&model, &[WireApi::Responses], &scope)
                    })
            })
            .map(|account| account.id.clone())
            .collect()
    }

    pub(crate) fn api_source_candidate_ids(&self) -> HashSet<String> {
        self.source_candidate_bindings.keys().cloned().collect()
    }

    /// Model information comes from the shared reference catalog, never a
    /// participant's optional capability fields.
    pub fn model_reasoning_levels(&self, model: &str) -> Vec<String> {
        crate::canonicalize_reasoning_levels(self.model_capabilities(model).reasoning_effort_levels)
    }

    pub(crate) fn codex_model_display_name(&self, model: &str) -> String {
        self.model_metadata_catalog
            .as_ref()
            .map(|catalog| catalog.snapshot().codex_display_name(model))
            .unwrap_or_else(|| crate::codex_model_display_name(model))
    }

    pub(crate) fn model_capabilities(
        &self,
        model: &str,
    ) -> crate::model_metadata::ModelCapabilities {
        self.model_metadata_catalog
            .as_ref()
            .map(|catalog| catalog.snapshot().capabilities_for(model))
            .unwrap_or_else(crate::model_metadata::ModelCapabilities::unknown_model)
    }

    pub(crate) fn model_reasoning_policy_levels(&self, model: &str) -> Option<Vec<String>> {
        let configured = crate::poison::mutex(&self.model_reasoning_allowed_levels);
        reasoning_policy_levels(&configured, model).map(ToOwned::to_owned)
    }

    pub(crate) fn client_reasoning_levels(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        client: WireApi,
    ) -> Vec<String> {
        let mut levels = Vec::new();
        for route in self.configured_executor_routes(key, model, &[client], false) {
            let capabilities = self
                .model_capabilities(&route.source_model)
                .for_route(route.route_capability.as_ref());
            let fallback = &capabilities.reasoning_effort_levels;
            levels.extend(
                fallback
                    .iter()
                    .filter(|level| {
                        self.basis_points_reasoning_available(
                            &route.candidate_id,
                            &route.source_model,
                            level,
                        ) && route.adapter.supports_reasoning_effort_for_model(
                            route.reasoning_mode,
                            level,
                            &capabilities,
                        )
                    })
                    .cloned(),
            );
        }
        crate::canonicalize_reasoning_levels(levels)
    }

    pub fn set_model_reasoning_allowed_levels(
        &self,
        allowed_levels: BTreeMap<String, Vec<String>>,
    ) -> Result<()> {
        let allowed_levels = normalize_model_reasoning_allowed_levels(allowed_levels)
            .map_err(|message| Error::Validation(message.to_string()))?;
        *crate::poison::mutex(&self.model_reasoning_allowed_levels) = allowed_levels;
        Ok(())
    }
}

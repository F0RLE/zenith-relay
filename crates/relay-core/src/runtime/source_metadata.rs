use super::{AuthenticatedKey, GatewayRuntime};
use crate::catalog::{normalize_model_reasoning_allowed_levels, reasoning_policy_levels};
use crate::{CandidateKind, Error, Result, WireApi};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::Ordering;

impl GatewayRuntime {
    pub(crate) fn visible_account_models(&self, key: &AuthenticatedKey) -> Vec<String> {
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let mut models = BTreeSet::new();
        for account in self.chatgpt_accounts.values() {
            let Some(candidate) = scheduler.candidate(&account.id) else {
                continue;
            };
            let inventory = account
                .model_inventory
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for model in &inventory.configured_models {
                if key.model_rules.allows(model)
                    && candidate.is_catalog_visible(model, &[WireApi::Responses], &scope)
                {
                    models.insert(match key.model_prefix.as_deref() {
                        Some(prefix) => format!("{prefix}/{model}"),
                        None => model.clone(),
                    });
                }
            }
        }
        models.into_iter().collect()
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
                account
                    .model_inventory
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
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
                !account.basis_points_enabled.load(Ordering::Relaxed)
                    && account
                        .model_inventory
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    /// Returns whether the key has at least one Responses route other than
    /// the explicitly labelled Excel / Basis Points transport. The shared
    /// speed policy can be projected for that model only when such a route
    /// exists; Basis Points alone is standard speed.
    pub(crate) fn codex_model_has_non_basis_responses_route(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> bool {
        let Some(model) = self.resolve_model(key, model) else {
            return false;
        };
        self.configured_executor_routes(key, &model, &[WireApi::Responses], false)
            .iter()
            .any(|route| {
                route.account_transport != crate::runtime::AccountTransport::ExcelBasisPoints
            })
    }

    /// Responses Lite is a whole-request transport contract, not a property
    /// of an individual fallback candidate. Automatic Lite is safe only when
    /// every configured route in this key scope is an official Codex account
    /// with confirmed Lite support for this exact model. Otherwise an attempt
    /// could switch tools or reasoning context between Lite and full Responses.
    ///
    /// This deliberately checks configured routes rather than current health,
    /// so a temporary cooldown cannot silently change the request contract.
    pub(crate) fn codex_model_responses_routes_all_support_lite(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> bool {
        self.codex_model_configured_responses_routes_all_support_lite(key, model, false)
    }

    /// Equivalent to [`Self::codex_model_responses_routes_all_support_lite`]
    /// for account-only endpoints. Generic API routes are intentionally
    /// excluded because those endpoints never select them.
    pub(crate) fn codex_model_account_responses_routes_all_support_lite(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> bool {
        self.codex_model_configured_responses_routes_all_support_lite(key, model, true)
    }

    fn codex_model_configured_responses_routes_all_support_lite(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        account_only: bool,
    ) -> bool {
        let Some(model) = self.resolve_model(key, model) else {
            return false;
        };
        let scope = key.scope_snapshot();
        // Do not hold the scheduler lock while inspecting catalog metadata.
        // Metadata can refresh independently, and a conservative false result
        // is preferable to blocking route selection.
        let configured = {
            let scheduler = self.lock_scheduler();
            scheduler
                .candidates()
                .filter(|candidate| {
                    candidate.is_configured(&model, &[WireApi::Responses], &scope)
                        && (!account_only || candidate.kind == CandidateKind::OAuthAccount)
                })
                .map(|candidate| (candidate.id.clone(), candidate.kind))
                .collect::<Vec<_>>()
        };
        if configured.is_empty() {
            return false;
        }
        let model = model.to_ascii_lowercase();
        let lite_models = self
            .codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        configured.into_iter().all(|(candidate_id, kind)| {
            kind == CandidateKind::OAuthAccount
                && lite_models.contains(&(candidate_id, model.clone()))
        })
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
        let configured = self
            .model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reasoning_policy_levels(&configured, model).map(ToOwned::to_owned)
    }

    pub(crate) fn client_reasoning_levels(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        client: WireApi,
    ) -> Vec<String> {
        let fallback = self.model_capabilities(model).reasoning_effort_levels;
        let mut levels = Vec::new();
        for route in self.configured_executor_routes(key, model, &[client], false) {
            levels.extend(
                fallback
                    .iter()
                    .filter(|level| {
                        route
                            .adapter
                            .supports_reasoning_effort(route.reasoning_mode, level)
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
        *self
            .model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = allowed_levels;
        Ok(())
    }
}

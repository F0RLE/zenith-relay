//! Model visibility for one authenticated key.
//!
//! Route eligibility stays with the scheduler. This module only resolves the
//! names a key is allowed to see or send.
use super::*;
use crate::DefaultServiceTier;

impl GatewayRuntime {
    pub(crate) fn resolve_model(&self, key: &AuthenticatedKey, model: &str) -> Option<String> {
        let model = model.trim();
        if model.is_empty() {
            return None;
        }
        let model = match key.model_prefix.as_deref() {
            Some(prefix) => strip_prefix_ignore_ascii_case(model, &format!("{prefix}/"))?,
            None => model,
        };
        if self.degraded_route_blocked(model) {
            return None;
        }
        (key.model_rules.allows(model) && self.model_enabled(model)).then(|| model.to_string())
    }

    pub(super) fn model_enabled(&self, model: &str) -> bool {
        !crate::poison::read(&self.hidden_models).contains(&crate::model_id_key(model))
    }

    /// Apply global visibility without replacing the scheduler or interrupting
    /// attempts that have already started upstream.
    pub fn set_hidden_models(&self, models: Vec<String>) {
        let hidden = models
            .iter()
            .map(|model| crate::model_id_key(model))
            .filter(|model| !model.is_empty())
            .collect();
        *crate::poison::write(&self.hidden_models) = hidden;
        self.candidate_availability.notify_waiters();
        self.admission_changed.notify_waiters();
    }

    pub(crate) fn resolve_visible_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Option<String> {
        let visible = self.visible_models(key, allowed_protocols, now_ms);
        self.resolve_from_visible(key, model, &visible)
    }

    /// Resolves a model that belongs to at least one configured route even
    /// when every such route is temporarily hidden by runtime health. This is
    /// deliberately narrower than `resolve_model`: unknown model ids must
    /// still fail admission instead of occupying a retry window.
    pub(crate) fn resolve_configured_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
    ) -> Option<String> {
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            scheduler
                .candidates()
                .any(|candidate| candidate.is_configured(&resolved, allowed_protocols, &scope))
                .then_some(resolved)
        };
        resolve(model).or_else(|| decode_codex_model_alias(model).and_then(|id| resolve(&id)))
    }

    pub(crate) fn resolve_visible_account_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Option<String> {
        self.resolve_from_visible(key, model, &self.visible_account_models(key))
    }

    pub(crate) fn resolve_configured_account_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Option<String> {
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            (!self
                .codex_model_chatgpt_account_ids_for_resolved(key, &resolved)
                .is_empty())
            .then_some(resolved)
        };
        resolve(model).or_else(|| decode_codex_model_alias(model).and_then(|id| resolve(&id)))
    }

    fn resolve_from_visible(
        &self,
        key: &AuthenticatedKey,
        requested: &str,
        visible: &[String],
    ) -> Option<String> {
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            visible
                .iter()
                .filter_map(|visible| self.resolve_model(key, visible))
                .any(|visible| visible.eq_ignore_ascii_case(&resolved))
                .then_some(resolved)
        };
        resolve(requested)
            .or_else(|| decode_codex_model_alias(requested).and_then(|id| resolve(&id)))
    }

    pub(crate) fn visible_models(
        &self,
        key: &AuthenticatedKey,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<String> {
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let mut models = crate::poison::mutex(&self.registry)
            .visible_models(&scheduler, &scope, allowed_protocols, now_ms)
            .into_iter()
            .filter(|model| {
                !self.degraded_route_blocked(model)
                    && key.model_rules.allows(model)
                    && self.model_enabled(model)
            })
            .collect::<Vec<_>>();
        let order = crate::poison::mutex(&self.model_display_order);
        models = self.model_metadata_catalog.as_ref().map_or_else(
            || crate::normalize_model_ids(models.iter()),
            |catalog| {
                catalog
                    .snapshot()
                    .merge_display_order(models.iter(), &order)
            },
        );
        models
            .into_iter()
            .map(|model| match key.model_prefix.as_deref() {
                Some(prefix) => format!("{prefix}/{model}"),
                None => model,
            })
            .collect()
    }

    pub(crate) async fn codex_models_routes(
        &self,
        key: &AuthenticatedKey,
        now_ms: u64,
    ) -> Vec<(String, Url)> {
        let scope = key.scope_snapshot();
        let routes = {
            let scheduler = self.lock_scheduler();
            self.chatgpt_accounts
                .values()
                .filter_map(|account| {
                    let candidate = scheduler.candidate(&account.id)?;
                    let inventory = crate::poison::read(&account.model_inventory);
                    let visible_models = inventory
                        .configured_models
                        .iter()
                        .filter(|model| {
                            key.model_rules.allows(model)
                                && self.model_enabled(model)
                                && candidate.is_catalog_visible(
                                    model,
                                    &[WireApi::Responses],
                                    &scope,
                                )
                        })
                        .count();
                    if visible_models == 0 {
                        return None;
                    }
                    let mut url = account.responses_url.clone();
                    let mut segments = url.path_segments_mut().ok()?;
                    segments.pop_if_empty().pop().push("models");
                    drop(segments);
                    Some((account.id.clone(), url, visible_models))
                })
                .collect::<Vec<_>>()
        };
        let mut ranked = Vec::with_capacity(routes.len());
        for (account_id, url, visible_models) in routes {
            let Some(account) = self.chatgpt_accounts.get(&account_id) else {
                continue;
            };
            let auth_state = account.token_authority.auth_state(&account_id).await;
            let tokens = account.token_authority.tokens(&account_id).await;
            let can_prepare =
                auth_state.is_none_or(|auth_state| !auth_state.requires_fresh_login());
            let token_rank = match tokens {
                Some(tokens)
                    if can_prepare && tokens.is_access_usable(now_ms, account.refresh_skew_ms) =>
                {
                    2_u8
                }
                Some(tokens) if can_prepare && tokens.refresh_token().is_some() => 1_u8,
                _ => 0_u8,
            };
            ranked.push((account_id, url, visible_models, token_rank));
        }
        ranked.sort_by(|left, right| {
            right
                .3
                .cmp(&left.3)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| left.0.cmp(&right.0))
        });
        ranked
            .into_iter()
            .map(|(account_id, url, _, _)| (account_id, url))
            .collect()
    }

    pub fn visible_models_for_secret(
        &self,
        secret: &str,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<String> {
        let Some(key) = self.authenticate_secret(secret) else {
            return Vec::new();
        };
        self.visible_models(&key, allowed_protocols, now_ms)
    }
}

impl GatewayRuntime {
    pub fn model_supports_service_tier(&self, model: &str, tier: DefaultServiceTier) -> bool {
        self.model_supported_service_tiers(model).contains(&tier)
    }

    pub(crate) fn model_supported_service_tiers(
        &self,
        model: &str,
    ) -> &'static [DefaultServiceTier] {
        self.model_metadata_catalog.as_ref().map_or_else(
            || crate::catalog::model_service_tiers(model, None),
            |catalog| catalog.snapshot().service_tiers_for(model),
        )
    }

    /// Applying a pool preference is a model policy. Retrying on another
    /// account or source never downgrades the selected mode.
    pub(crate) fn project_service_tier_for_model(
        &self,
        model: &str,
        requested: DefaultServiceTier,
    ) -> DefaultServiceTier {
        if self.model_supports_service_tier(model, requested) {
            requested
        } else {
            DefaultServiceTier::Standard
        }
    }
}

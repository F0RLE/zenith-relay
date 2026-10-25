//! Model visibility for one authenticated key.
//!
//! Route eligibility stays with the scheduler. This module only resolves the
//! names a key is allowed to see or send.
use super::*;
use crate::DefaultServiceTier;

impl GatewayRuntime {
    /// Resolve Desktop's reserved client namespace against configured model
    /// identities only. Callers still apply key scope, rules, visibility and
    /// route eligibility to the result. Never fall back to a raw provider model
    /// with an alias-shaped name: it could shadow a different model's route.
    fn desktop_requested_model<'a>(
        &self,
        key: &AuthenticatedKey,
        requested: &'a str,
    ) -> Option<std::borrow::Cow<'a, str>> {
        let requested = requested.trim();
        if !crate::catalog::is_claude_desktop_model_alias(requested) {
            return Some(std::borrow::Cow::Borrowed(requested));
        }
        let scheduler = self.lock_scheduler();
        let mut resolved: Option<String> = None;
        for model in scheduler
            .candidates()
            .flat_map(|candidate| &candidate.models)
        {
            let displayed = match key.model_prefix.as_deref() {
                Some(prefix) => format!("{prefix}/{model}"),
                None => model.clone(),
            };
            if !crate::catalog::claude_desktop_model_alias_matches(requested, &displayed) {
                continue;
            }
            if let Some(previous) = &resolved {
                // A compact-hash collision must fail closed, never choose by pool order.
                if crate::model_id_key(previous) != crate::model_id_key(&displayed) {
                    return None;
                }
            } else {
                resolved = Some(displayed);
            }
        }
        resolved.map(std::borrow::Cow::Owned)
    }

    pub fn set_model_context_policies(
        &self,
        policies: std::collections::BTreeMap<String, crate::ModelContextPolicy>,
    ) -> crate::Result<()> {
        let policies = crate::normalize_model_context_policies(policies)
            .map_err(|message| crate::Error::Validation(message.to_string()))?;
        *crate::poison::mutex(&self.model_context_policies) = policies;
        Ok(())
    }

    pub(crate) fn apply_model_context(
        &self,
        entry: &mut Value,
        model: &str,
        native: bool,
        native_card: bool,
    ) {
        let selected = crate::poison::mutex(&self.model_context_policies)
            .get(&crate::model_id_key(model))
            .copied()
            .unwrap_or_default();
        // A native account without an exact model card has no authoritative
        // context window. Keep the identity-only fallback untouched unless
        // the user explicitly selected a custom window for this model.
        if native && !native_card && !selected.is_custom() {
            return;
        }
        let capabilities = self.model_capabilities(model);
        if !crate::catalog::context_policy::model_supports_text_context(
            model,
            None,
            &capabilities.output_modalities,
        ) {
            return;
        }
        if native {
            crate::catalog::context_policy::apply_native_model_context(
                entry,
                selected,
                capabilities.input_limit,
                capabilities.output_limit,
            );
        } else {
            crate::catalog::context_policy::apply_model_context(
                entry,
                selected,
                capabilities.context_limit,
                capabilities.input_limit,
                capabilities.output_limit,
            );
        }
    }

    pub(crate) fn resolve_model(&self, key: &AuthenticatedKey, model: &str) -> Option<String> {
        let requested_model_id = model.trim();
        if requested_model_id.is_empty() {
            return None;
        }
        let scoped_model_id = match key.model_prefix.as_deref() {
            Some(prefix) => {
                strip_prefix_ignore_ascii_case(requested_model_id, &format!("{prefix}/"))?
            }
            None => requested_model_id,
        };
        if self.degraded_route_blocked(scoped_model_id) {
            return None;
        }
        (key.model_rules.allows(scoped_model_id) && self.model_enabled(scoped_model_id))
            .then(|| scoped_model_id.to_string())
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
        let desktop_model = self.desktop_requested_model(key, model)?;
        let model = desktop_model.as_ref();
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            scheduler
                .candidates()
                .any(|candidate| candidate.is_configured(&resolved, allowed_protocols, &scope))
                .then_some(resolved)
        };
        resolve(model)
            .or_else(|| decode_codex_model_alias(model).and_then(|model_id| resolve(&model_id)))
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
        let desktop_model = self.desktop_requested_model(key, model)?;
        let model = desktop_model.as_ref();
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            (!self
                .codex_model_chatgpt_account_ids_for_resolved(key, &resolved)
                .is_empty())
            .then_some(resolved)
        };
        resolve(model)
            .or_else(|| decode_codex_model_alias(model).and_then(|model_id| resolve(&model_id)))
    }

    fn resolve_from_visible(
        &self,
        key: &AuthenticatedKey,
        requested: &str,
        visible: &[String],
    ) -> Option<String> {
        let desktop_model = self.desktop_requested_model(key, requested)?;
        let requested = desktop_model.as_ref();
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            visible
                .iter()
                .filter_map(|visible_model_id| self.resolve_model(key, visible_model_id))
                .any(|visible_model_id| visible_model_id.eq_ignore_ascii_case(&resolved))
                .then_some(resolved)
        };
        resolve(requested)
            .or_else(|| decode_codex_model_alias(requested).and_then(|model_id| resolve(&model_id)))
    }

    pub(crate) fn visible_models(
        &self,
        key: &AuthenticatedKey,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<String> {
        let scope = key.scope_snapshot();
        let basis_points_models = self.basis_points_model_snapshot();
        let scheduler = self.lock_scheduler();
        let mut models = crate::poison::mutex(&self.registry)
            .visible_models(&scheduler, &scope, allowed_protocols, now_ms)
            .into_iter()
            .filter(|model| {
                !self.degraded_route_blocked(model)
                    && key.model_rules.allows(model)
                    && self.model_enabled(model)
                    && scheduler.candidates().any(|candidate| {
                        candidate.is_catalog_visible(model, allowed_protocols, &scope)
                            && basis_points_models.model_available(&candidate.id, model)
                    })
            })
            .collect::<Vec<_>>();
        let order = crate::poison::mutex(&self.model_display_order);
        let catalog = self.model_metadata_catalog.as_ref().map_or_else(
            || Arc::new(crate::model_metadata::ModelMetadataCatalog::empty()),
            |catalog| catalog.snapshot(),
        );
        models = catalog.merge_display_order(models.iter(), &order);
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
                    if self.is_basis_points_account(&account.id) {
                        return Some((
                            account.id.clone(),
                            crate::providers::chatgpt::basis_points_access_url(
                                &account.basis_points_url,
                            )?,
                            visible_models,
                        ));
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

    pub(crate) fn model_supported_service_tiers(&self, model: &str) -> Vec<DefaultServiceTier> {
        let tiers = crate::catalog::model_service_tiers(
            model,
            &self.model_capabilities(model).service_tiers,
        );
        // Standard is unconditional in service_tiers_for_routes. If reference
        // metadata advertises no optional tier, resolving every executor route
        // cannot change this result (including for an empty/offline pool).
        if tiers
            .iter()
            .all(|tier| *tier == DefaultServiceTier::Standard)
        {
            return tiers;
        }
        let scope = CandidateScope::default();
        let candidate_ids = self
            .lock_scheduler()
            .candidates()
            .filter(|candidate| candidate.is_configured(model, &WireApi::ALL_NATIVE, &scope))
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let routes = candidate_ids
            .iter()
            .filter_map(|id| self.executor_route(id, model, &scope, &WireApi::ALL_NATIVE, false))
            .collect::<Vec<_>>();
        self.service_tiers_for_routes(model, &routes)
    }

    pub(crate) fn client_service_tiers(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        client: WireApi,
    ) -> Vec<DefaultServiceTier> {
        self.service_tiers_for_routes(
            model,
            &self.configured_executor_routes(key, model, &[client], false),
        )
    }

    pub(crate) fn account_service_tiers(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Vec<DefaultServiceTier> {
        let routes = self
            .configured_executor_routes(key, model, &[WireApi::Responses], false)
            .into_iter()
            .filter(|route| route.account_id.is_some())
            .collect::<Vec<_>>();
        self.service_tiers_for_routes(model, &routes)
    }

    fn service_tiers_for_routes(
        &self,
        model: &str,
        routes: &[ExecutorRoute],
    ) -> Vec<DefaultServiceTier> {
        crate::catalog::model_service_tiers(model, &self.model_capabilities(model).service_tiers)
            .into_iter()
            .filter(|tier| {
                *tier == DefaultServiceTier::Standard
                    || routes
                        .iter()
                        .any(|route| Self::route_supports_service_tier(route, *tier))
            })
            .collect()
    }

    pub(crate) fn route_supports_service_tier(
        route: &ExecutorRoute,
        tier: DefaultServiceTier,
    ) -> bool {
        crate::catalog::transport_supports_service_tier(
            tier,
            route
                .adapter
                .upstream_protocol(route.client_wire_api)
                .wire_api(),
            route.account_transport == AccountTransport::ExcelBasisPoints,
            route
                .route_capability
                .as_ref()
                .and_then(|route| route.service_tiers.as_deref()),
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

use super::{
    confirmed_source_reasoning_levels as merge_confirmed_source_reasoning_levels, runtime_now_ms,
    source_reasoning_for_route, AuthenticatedKey, CachedModelManifest, CodexSourceModelMetadata,
    GatewayRuntime, SourceModelMetadataPrefetchGuard, CODEX_SOURCE_MODEL_MANIFEST_TTL_MS,
    SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS,
};
use crate::catalog::{
    apply_claude_reasoning_capability_fallback, normalize_model_reasoning_allowed_levels,
    source_context_windows, source_image_input_capabilities, source_reasoning_capabilities,
    union_source_reasoning_capabilities, SourceReasoningCapabilities,
};
use crate::transport::{collect_limited, MAX_MODEL_CATALOG_BODY_BYTES};
use crate::{
    CandidateScope, Error, MessagesReasoningMode, ModelRules, Result, SourceAdapter, WireApi,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use futures_util::future::join_all;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

struct SourceMetadataRoute {
    candidate_id: String,
    models_url: Url,
    authorization_name: HeaderName,
    authorization: HeaderValue,
    protocol_headers: HeaderMap,
    configured_models: BTreeSet<String>,
    adapter: SourceAdapter,
    reasoning_mode: MessagesReasoningMode,
}

struct SourceMetadataManifest {
    candidate_id: String,
    configured_models: BTreeSet<String>,
    adapter: SourceAdapter,
    reasoning_mode: MessagesReasoningMode,
    manifest: Option<Value>,
}

impl GatewayRuntime {
    /// Starts a best-effort metadata refresh for the management UI. The result
    /// arrives on the next state poll and never delays the current one.
    pub fn prefetch_source_model_metadata(self: &Arc<Self>) {
        if runtime_now_ms()
            < self
                .model_metadata
                .prefetch_not_before_ms
                .load(Ordering::Acquire)
        {
            return;
        }
        if self
            .model_metadata
            .prefetch_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let pending_guard = SourceModelMetadataPrefetchGuard {
            runtime: Arc::clone(self),
        };
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let _pending_guard = pending_guard;
            let rules = ModelRules::default();
            let scope = runtime.management_source_metadata_scope();
            runtime
                .source_model_metadata(&rules, &scope, &[WireApi::Responses], runtime_now_ms())
                .await;
            runtime.model_metadata.prefetch_not_before_ms.store(
                runtime_now_ms().saturating_add(SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS),
                Ordering::Release,
            );
        });
    }

    /// Builds the source portion of the scope represented by active gateway
    /// credentials. Management must not advertise an effort discovered on an
    /// API source that no active pool key can reach.
    fn management_source_metadata_scope(&self) -> CandidateScope {
        let mut source_ids = BTreeSet::new();
        for key in self.keys.iter().filter(|key| key.enabled) {
            let scope = key
                .scope
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if scope.source_ids.is_none() && scope.account_ids.is_none() {
                return CandidateScope::default();
            }
            if let Some(ids) = scope.source_ids.as_ref() {
                source_ids.extend(ids.iter().cloned());
            }
        }
        CandidateScope {
            source_ids: Some(source_ids),
            account_ids: Some(BTreeSet::new()),
            model_rules: ModelRules::default(),
        }
    }

    pub(super) async fn source_model_metadata(
        &self,
        model_rules: &ModelRules,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> CodexSourceModelMetadata {
        let routes = self.source_metadata_routes(model_rules, scope, allowed_protocols);
        let manifests = self.source_metadata_manifests(routes, now_ms).await;

        let mut metadata = CodexSourceModelMetadata::default();
        let mut reasoning_by_model = BTreeMap::<String, Vec<SourceReasoningCapabilities>>::new();
        let mut evaluated_reasoning = Vec::<(String, String, BTreeSet<String>)>::new();
        let mut image_support_by_model = BTreeMap::<String, Vec<bool>>::new();
        for SourceMetadataManifest {
            candidate_id,
            configured_models,
            adapter,
            reasoning_mode,
            manifest,
        } in manifests
        {
            let reasoning = manifest
                .as_ref()
                .map(|manifest| source_reasoning_capabilities(manifest, &configured_models))
                .unwrap_or_default();
            let declared_image_support = manifest
                .as_ref()
                .map(|manifest| source_image_input_capabilities(manifest, &configured_models))
                .unwrap_or_default();
            if let Some(manifest) = manifest.as_ref() {
                for (model, context_window) in source_context_windows(manifest, &configured_models)
                {
                    metadata
                        .context_windows
                        .entry(model)
                        .and_modify(|existing| *existing = (*existing).min(context_window))
                        .or_insert(context_window);
                }
            }
            for model in &configured_models {
                let model_key = model.to_ascii_lowercase();
                let supports_image = matches!(adapter, SourceAdapter::ResponsesToMessages)
                    || declared_image_support
                        .get(&model_key)
                        .copied()
                        .unwrap_or(false);
                image_support_by_model
                    .entry(model_key.clone())
                    .or_default()
                    .push(supports_image);
                let capabilities = apply_claude_reasoning_capability_fallback(
                    model,
                    reasoning.get(&model_key).cloned(),
                )
                .and_then(|capabilities| {
                    source_reasoning_for_route(capabilities, adapter, reasoning_mode)
                });
                if manifest.is_some() {
                    evaluated_reasoning.push((
                        model_key.clone(),
                        candidate_id.clone(),
                        capabilities
                            .as_ref()
                            .map(|capabilities| {
                                capabilities
                                    .effort_ids()
                                    .map(str::to_ascii_lowercase)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ));
                }
                if let Some(capabilities) = capabilities {
                    reasoning_by_model
                        .entry(model_key)
                        .or_default()
                        .push(capabilities);
                }
            }
        }
        let mut current_reasoning_levels = BTreeMap::new();
        for (model, capabilities) in reasoning_by_model {
            let Some(capabilities) = union_source_reasoning_capabilities(capabilities) else {
                continue;
            };
            current_reasoning_levels.insert(
                model.clone(),
                capabilities
                    .effort_ids()
                    .map(str::to_ascii_lowercase)
                    .collect(),
            );
            metadata
                .reasoning_catalog_templates
                .insert(model, capabilities.codex_catalog_template());
        }
        {
            let mut confirmed = self
                .model_metadata
                .confirmed_reasoning
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (model, candidate_id, efforts) in evaluated_reasoning {
                if efforts.is_empty() {
                    let remove_model = confirmed.efforts.get_mut(&model).is_some_and(|routes| {
                        routes.remove(&candidate_id);
                        routes.is_empty()
                    });
                    if remove_model {
                        confirmed.efforts.remove(&model);
                    }
                } else {
                    confirmed
                        .efforts
                        .entry(model)
                        .or_default()
                        .insert(candidate_id, efforts);
                }
            }
            let previous_levels = confirmed.levels.clone();
            confirmed.levels = merge_confirmed_source_reasoning_levels(
                &confirmed.efforts,
                &previous_levels,
                &current_reasoning_levels,
            );
        }
        for (model, route_support) in image_support_by_model {
            if route_support.iter().all(|supports_image| *supports_image) {
                metadata.image_models.insert(model);
            }
        }
        metadata
    }

    fn source_metadata_routes(
        &self,
        model_rules: &ModelRules,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
    ) -> Vec<SourceMetadataRoute> {
        let scheduler = self.lock_scheduler();
        let mut routes = Vec::new();
        for (candidate_id, binding) in &self.source_candidate_bindings {
            if binding.wire_api != WireApi::Responses {
                continue;
            }
            let Some(candidate) = scheduler.candidate(candidate_id) else {
                continue;
            };
            let Some(source) = self.sources.get(&binding.source_id) else {
                continue;
            };
            let Some(models) = source.models_for(binding.binding_key) else {
                continue;
            };
            let configured_models = models
                .iter()
                .filter(|model| {
                    model_rules.allows(model)
                        && candidate.is_catalog_visible(model, allowed_protocols, scope)
                })
                .cloned()
                .collect::<BTreeSet<_>>();
            if configured_models.is_empty() {
                continue;
            }
            let Some(source_binding) = source.binding_for(binding.binding_key) else {
                continue;
            };
            let (authorization_name, authorization) =
                source.authorization_for_binding(source_binding);
            routes.push(SourceMetadataRoute {
                candidate_id: candidate_id.clone(),
                models_url: source.models_url.clone(),
                authorization_name,
                authorization,
                protocol_headers: source.protocol_headers_for_binding(source_binding),
                configured_models,
                adapter: binding.adapter,
                reasoning_mode: binding.reasoning_mode,
            });
        }
        routes
    }

    async fn source_metadata_manifests(
        &self,
        routes: Vec<SourceMetadataRoute>,
        now_ms: u64,
    ) -> Vec<SourceMetadataManifest> {
        // A catalog request with fresh manifests must not queue behind an
        // unrelated best-effort refresh. Stale callers still serialize their
        // upstream discovery so they coalesce onto one cache refill.
        let refresh_required = routes.iter().any(|route| {
            self.cached_source_model_manifest(&route.candidate_id)
                .is_none_or(|manifest| {
                    now_ms.saturating_sub(manifest.observed_at_ms)
                        > CODEX_SOURCE_MODEL_MANIFEST_TTL_MS
                })
        });
        let refresh_guard = if refresh_required {
            Some(self.model_metadata.refresh_lock.lock().await)
        } else {
            None
        };
        let manifests = join_all(
            routes
                .into_iter()
                .map(|route| self.source_metadata_manifest(route, now_ms)),
        )
        .await;
        drop(refresh_guard);
        manifests
    }

    async fn source_metadata_manifest(
        &self,
        route: SourceMetadataRoute,
        now_ms: u64,
    ) -> SourceMetadataManifest {
        let SourceMetadataRoute {
            candidate_id,
            models_url,
            authorization_name,
            authorization,
            protocol_headers,
            configured_models,
            adapter,
            reasoning_mode,
        } = route;
        let cached_manifest = self.cached_source_model_manifest(&candidate_id);
        let manifest = if cached_manifest.as_ref().is_some_and(|manifest| {
            now_ms.saturating_sub(manifest.observed_at_ms) <= CODEX_SOURCE_MODEL_MANIFEST_TTL_MS
        }) {
            cached_manifest.map(|manifest| manifest.value)
        } else {
            let fetched_manifest = async {
                let response = self
                    .discovery_client
                    .get(models_url)
                    .headers(protocol_headers)
                    .header(authorization_name, authorization)
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await
                    .ok()?;
                if !response.status().is_success() {
                    return None;
                }
                let body = collect_limited(response, MAX_MODEL_CATALOG_BODY_BYTES)
                    .await
                    .ok()?;
                serde_json::from_slice::<Value>(&body).ok()
            }
            .await;
            if let Some(value) = fetched_manifest {
                self.remember_source_model_manifest(&candidate_id, value.clone(), now_ms);
                Some(value)
            } else {
                // A transient discovery failure must not make a model appear to
                // lose its previously confirmed capabilities. Candidate removal
                // and configured model filters still take effect before this
                // cache is considered.
                cached_manifest.map(|manifest| manifest.value)
            }
        };
        SourceMetadataManifest {
            candidate_id,
            configured_models,
            adapter,
            reasoning_mode,
            manifest,
        }
    }

    pub(crate) fn set_codex_model_uses_responses_lite(&self, model: &str, enabled: bool) {
        let mut models = self
            .codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if enabled {
            models.insert(model.to_ascii_lowercase());
        } else {
            models.remove(&model.to_ascii_lowercase());
        }
    }

    pub(crate) fn codex_model_uses_responses_lite(&self, model: &str) -> bool {
        self.codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&model.to_ascii_lowercase())
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

    pub(super) fn remember_source_model_manifest(
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
            .source_manifests
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

    fn cached_source_model_manifest(&self, candidate_id: &str) -> Option<CachedModelManifest> {
        self.model_metadata
            .source_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(candidate_id)
            .cloned()
    }

    pub(crate) fn stale_codex_model_manifests<'a>(
        &self,
        candidate_ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<Value> {
        let manifests = self
            .model_metadata
            .codex_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        candidate_ids
            .into_iter()
            .filter_map(|candidate_id| manifests.get(candidate_id))
            .map(|manifest| manifest.value.clone())
            .collect()
    }

    pub(crate) fn visible_account_models(&self, key: &AuthenticatedKey) -> Vec<String> {
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let mut models = BTreeSet::new();
        for account in self.chatgpt_accounts.values() {
            let Some(candidate) = scheduler.candidate(&account.id) else {
                continue;
            };
            for model in &account.configured_models {
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

    /// A bare Codex model keeps native metadata whenever a configured
    /// ChatGPT account can serve it.  Generic sources may expose the same
    /// upstream-looking id, but they must not downgrade the native account
    /// entry in the pool catalog.
    ///
    /// This deliberately checks configured routes rather than current health:
    /// a temporary catalogue-discovery failure must not erase native
    /// capabilities from a saved ChatGPT manifest.
    pub(crate) fn codex_model_has_chatgpt_account(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> bool {
        let Some(model) = self.resolve_model(key, model) else {
            return false;
        };
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        self.chatgpt_accounts.values().any(|account| {
            account
                .configured_models
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&model))
                && scheduler.candidate(&account.id).is_some_and(|candidate| {
                    candidate.is_configured(&model, &[WireApi::Responses], &scope)
                })
        })
    }

    pub(crate) fn api_source_candidate_ids(&self) -> HashSet<String> {
        self.source_candidate_bindings.keys().cloned().collect()
    }

    /// Excludes API routes that have not explicitly confirmed support for an
    /// effort. Native ChatGPT routes are deliberately not part of this set:
    /// their request and catalog semantics remain provider-owned.
    pub(crate) fn exclude_api_sources_without_reasoning_effort(
        &self,
        model: &str,
        effort: &str,
        tried: &mut HashSet<String>,
    ) {
        let model = model.trim().to_ascii_lowercase();
        let effort = effort.trim().to_ascii_lowercase();
        if model.is_empty() || effort.is_empty() || effort == "none" {
            return;
        }
        let confirmed = self
            .model_metadata
            .confirmed_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A catalog refresh can be absent or temporarily fail. Until at least
        // one route has confirmed capabilities for this model, preserve the
        // normal transparent fallback rather than excluding every source.
        let Some(confirmed) = confirmed.efforts.get(&model) else {
            return;
        };
        for candidate_id in self.source_candidate_bindings.keys() {
            if !confirmed
                .get(candidate_id)
                .is_some_and(|efforts| efforts.contains(&effort))
            {
                tried.insert(candidate_id.clone());
            }
        }
    }

    pub fn confirmed_source_reasoning_levels(&self, model: &str) -> Vec<String> {
        self.model_metadata
            .confirmed_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .levels
            .get(&model.trim().to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// Returns a model's manually allowed reasoning levels. An empty result
    /// means the catalog remains automatic and exposes every confirmed level.
    pub fn model_reasoning_allowed_levels(&self, model: &str) -> Vec<String> {
        self.model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model.trim().to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub fn model_reasoning_effort_is_allowed(&self, model: &str, effort: &str) -> bool {
        let model = model.trim().to_ascii_lowercase();
        let effort = effort.trim().to_ascii_lowercase();
        self.model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model)
            .is_none_or(|allowed| allowed.iter().any(|level| level == &effort))
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

    pub fn supports_source_reasoning_effort(&self, model: &str, effort: &str) -> bool {
        self.model_metadata
            .confirmed_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .efforts
            .get(&model.trim().to_ascii_lowercase())
            .is_some_and(|routes| {
                routes
                    .values()
                    .any(|efforts| efforts.contains(&effort.trim().to_ascii_lowercase()))
            })
    }
}

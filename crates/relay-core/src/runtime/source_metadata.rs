use super::{
    declared_source_reasoning_levels as merge_declared_source_reasoning_levels, runtime_now_ms,
    source_reasoning_for_route, AuthenticatedKey, CachedModelManifest, CodexSourceModelMetadata,
    GatewayRuntime, SourceModelMetadataPrefetchGuard, CODEX_SOURCE_MODEL_MANIFEST_TTL_MS,
    SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS,
};
use crate::catalog::{
    normalize_model_reasoning_allowed_levels, reasoning_policy_levels, source_context_windows,
    source_image_input_capabilities, source_reasoning_capabilities,
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

const MANAGEMENT_SOURCE_METADATA_PROTOCOLS: &[WireApi] = &[
    WireApi::Responses,
    WireApi::ChatCompletions,
    WireApi::Messages,
    WireApi::Gemini,
];

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

/// Discovery callers either accept a fresh cached manifest or explicitly ask
/// to bypass the normal prefetch throttle. Keeping this mode explicit avoids
/// accidentally inverting a boolean at a catalog call site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceMetadataRefresh {
    Cached,
    Forced,
}

impl SourceMetadataRefresh {
    const fn is_forced(self) -> bool {
        matches!(self, Self::Forced)
    }
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
                .source_model_metadata(
                    &rules,
                    &scope,
                    MANAGEMENT_SOURCE_METADATA_PROTOCOLS,
                    runtime_now_ms(),
                )
                .await;
            runtime.model_metadata.prefetch_not_before_ms.store(
                runtime_now_ms().saturating_add(SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS),
                Ordering::Release,
            );
        });
    }

    /// Forces a source metadata refresh for an explicit management action.
    /// Unlike the background prefetch, this bypasses the normal eight-hour
    /// throttle while retaining the previous manifest as a fallback when the
    /// provider is temporarily unavailable.
    pub async fn refresh_source_model_metadata(self: &Arc<Self>) {
        let scope = self.management_source_metadata_scope();
        self.refresh_source_model_metadata_in_scope(scope).await;
    }

    /// Forces metadata discovery for one management source after its catalog
    /// was explicitly refreshed. Other sources retain their cached metadata,
    /// so a bulk source refresh scales with the number of changed sources
    /// instead of repeatedly rediscovering the complete pool.
    pub async fn refresh_source_model_metadata_for_source(self: &Arc<Self>, source_id: &str) {
        let mut scope = self.management_source_metadata_scope();
        scope.source_ids = Some(match scope.source_ids {
            Some(source_ids) if source_ids.contains(source_id) => [source_id.to_string()].into(),
            Some(_) => BTreeSet::new(),
            None => [source_id.to_string()].into(),
        });
        self.refresh_source_model_metadata_in_scope(scope).await;
    }

    async fn refresh_source_model_metadata_in_scope(self: &Arc<Self>, scope: CandidateScope) {
        let rules = ModelRules::default();
        self.source_model_metadata_with_refresh(
            &rules,
            &scope,
            MANAGEMENT_SOURCE_METADATA_PROTOCOLS,
            runtime_now_ms(),
            SourceMetadataRefresh::Forced,
        )
        .await;
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
            let Some(ids) = scope.source_ids.as_ref() else {
                // Account restrictions do not restrict API sources.
                return CandidateScope::default();
            };
            source_ids.extend(ids.iter().cloned());
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
        self.source_model_metadata_with_refresh(
            model_rules,
            scope,
            allowed_protocols,
            now_ms,
            SourceMetadataRefresh::Cached,
        )
        .await
    }

    async fn source_model_metadata_with_refresh(
        &self,
        model_rules: &ModelRules,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
        now_ms: u64,
        refresh: SourceMetadataRefresh,
    ) -> CodexSourceModelMetadata {
        let routes = self.source_metadata_routes(model_rules, scope, allowed_protocols);
        let manifests = self
            .source_metadata_manifests(routes, now_ms, refresh)
            .await;

        let mut metadata = CodexSourceModelMetadata::default();
        let mut reasoning_by_model = BTreeMap::<String, Vec<SourceReasoningCapabilities>>::new();
        let mut declared_reasoning = Vec::<(String, String, BTreeSet<String>, bool)>::new();
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
                let supports_image = matches!(
                    adapter,
                    SourceAdapter::ResponsesToMessages | SourceAdapter::ResponsesToGemini
                ) || declared_image_support
                    .get(&model_key)
                    .copied()
                    .unwrap_or(false);
                image_support_by_model
                    .entry(model_key.clone())
                    .or_default()
                    .push(supports_image);
                let capabilities = reasoning.get(&model_key).cloned().and_then(|capabilities| {
                    let mut capabilities =
                        source_reasoning_for_route(capabilities, adapter, reasoning_mode)?;
                    capabilities.apply_model_implied_efforts(&model_key);
                    Some(capabilities)
                });
                // Keep provider-declared modes as catalog metadata. A refresh
                // never becomes request admission evidence, and omission of
                // reasoning fields must not affect ordinary model routing.
                if reasoning.contains_key(&model_key) {
                    declared_reasoning.push((
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
                        capabilities
                            .as_ref()
                            .is_some_and(SourceReasoningCapabilities::is_empty),
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
        self.update_declared_source_reasoning(declared_reasoning, &current_reasoning_levels);
        for (model, route_support) in image_support_by_model {
            if route_support.iter().all(|supports_image| *supports_image) {
                metadata.image_models.insert(model);
            }
        }
        metadata
    }

    fn update_declared_source_reasoning(
        &self,
        declared_reasoning: Vec<(String, String, BTreeSet<String>, bool)>,
        current_reasoning_levels: &BTreeMap<String, Vec<String>>,
    ) {
        let mut declared = self
            .model_metadata
            .declared_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (model, candidate_id, efforts, explicitly_empty) in declared_reasoning {
            if efforts.is_empty() {
                let remove_model = declared.efforts.get_mut(&model).is_some_and(|routes| {
                    routes.remove(&candidate_id);
                    routes.is_empty()
                });
                if remove_model {
                    declared.efforts.remove(&model);
                }
                if explicitly_empty {
                    declared
                        .empty_routes
                        .entry(model)
                        .or_default()
                        .insert(candidate_id);
                } else if let Some(routes) = declared.empty_routes.get_mut(&model) {
                    routes.remove(&candidate_id);
                    if routes.is_empty() {
                        declared.empty_routes.remove(&model);
                    }
                }
            } else {
                if let Some(routes) = declared.empty_routes.get_mut(&model) {
                    routes.remove(&candidate_id);
                    if routes.is_empty() {
                        declared.empty_routes.remove(&model);
                    }
                }
                declared
                    .efforts
                    .entry(model)
                    .or_default()
                    .insert(candidate_id, efforts);
            }
        }
        let previous_levels = declared.levels.clone();
        declared.levels = merge_declared_source_reasoning_levels(
            &declared.efforts,
            &previous_levels,
            current_reasoning_levels,
        );
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
        refresh: SourceMetadataRefresh,
    ) -> Vec<SourceMetadataManifest> {
        let forced = refresh.is_forced();
        let initial_not_before_ms = self
            .model_metadata
            .prefetch_not_before_ms
            .load(Ordering::Acquire);
        let mut refresh_allowed = forced || now_ms >= initial_not_before_ms;
        // A catalog request with fresh manifests must not queue behind an
        // unrelated best-effort refresh. Stale callers still serialize their
        // upstream discovery so they coalesce onto one cache refill.
        let refresh_required = (forced || refresh_allowed)
            && routes.iter().any(|route| {
                self.cached_source_model_manifest(&route.candidate_id)
                    .is_none_or(|manifest| {
                        forced
                            || now_ms.saturating_sub(manifest.observed_at_ms)
                                > CODEX_SOURCE_MODEL_MANIFEST_TTL_MS
                    })
            });
        let refresh_guard = if refresh_required {
            Some(self.model_metadata.refresh_lock.lock().await)
        } else {
            None
        };
        // An explicit refresh bypasses the throttle for the caller that wins
        // the lock. If another explicit refresh was already waiting, the
        // winner's throttle update tells that waiter to use the newly cached
        // manifest instead of issuing a duplicate upstream request.
        let effective_refresh = if forced && refresh_guard.is_some() {
            let current_not_before_ms = self
                .model_metadata
                .prefetch_not_before_ms
                .load(Ordering::Acquire);
            if current_not_before_ms != initial_not_before_ms {
                refresh_allowed = now_ms >= current_not_before_ms;
                SourceMetadataRefresh::Cached
            } else {
                SourceMetadataRefresh::Forced
            }
        } else {
            refresh
        };
        let manifests = join_all(routes.into_iter().map(|route| {
            self.source_metadata_manifest(route, now_ms, refresh_allowed, effective_refresh)
        }))
        .await;
        drop(refresh_guard);
        if refresh_required {
            self.model_metadata.prefetch_not_before_ms.store(
                now_ms.saturating_add(SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS),
                Ordering::Release,
            );
        }
        manifests
    }

    async fn source_metadata_manifest(
        &self,
        route: SourceMetadataRoute,
        now_ms: u64,
        refresh_allowed: bool,
        refresh: SourceMetadataRefresh,
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
        let manifest = if !refresh_allowed
            || (!refresh.is_forced()
                && cached_manifest.as_ref().is_some_and(|manifest| {
                    now_ms.saturating_sub(manifest.observed_at_ms)
                        <= CODEX_SOURCE_MODEL_MANIFEST_TTL_MS
                })) {
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
                // lose its previously declared capabilities. Candidate removal
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
        !self.codex_model_chatgpt_account_ids(key, model).is_empty()
    }

    pub(crate) fn codex_model_chatgpt_account_ids(
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
                account
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

    pub fn declared_source_reasoning_levels(&self, model: &str) -> Vec<String> {
        self.model_metadata
            .declared_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .levels
            .get(&model.trim().to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub fn source_declared_reasoning_levels(&self, model: &str) -> Option<Vec<String>> {
        let model = model.trim().to_ascii_lowercase();
        let declared = self
            .model_metadata
            .declared_reasoning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        declared.levels.get(&model).cloned().or_else(|| {
            declared
                .empty_routes
                .get(&model)
                .filter(|routes| !routes.is_empty())
                .map(|_| Vec::new())
        })
    }

    pub(crate) fn model_reasoning_policy_levels(&self, model: &str) -> Option<Vec<String>> {
        let configured = self
            .model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reasoning_policy_levels(&configured, model).map(ToOwned::to_owned)
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

use super::{
    ChatGptAccountExecutor, GatewayRuntimeOptions, PassiveQuotaState, RuntimeHttpClients,
    RuntimeKey, RuntimeSource, SourceCandidateBinding, SourceCandidateRoute, all_native_wire_apis,
    client_wire_apis_to_native, model_rules, normalize_client_wire_api, normalize_prefix,
    normalized_set, source_candidate_id,
};
use super::{
    CodexTurnStateStore, GatewayRuntime, LocalGatewayKey, NEXT_ACTIVITY_RUNTIME_ID, ProviderSource,
    RuntimeControl, RuntimeLocalKey, SourceModelMetadataState, UsageCallback,
    normalize_image_base_model, runtime_now_ms,
};
use crate::catalog::normalize_model_reasoning_allowed_levels;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
use crate::{
    CandidateHealth, CandidateKind, CandidateQuota, CandidateScope, Error, ModelRegistry,
    ModelRules, PoolScheduler, Result, RuntimeCandidate, RuntimeMixedLocalKey, SourceConnector,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

mod accounts;
mod keys;
mod sources;
use keys::build_keys;
use sources::build_sources;
#[derive(Clone, Copy)]
pub(super) enum ReachabilityRequirement {
    RequireReachable,
    AllowUnroutable,
}

pub(super) struct SourceRuntimeParts {
    pub(super) executors: BTreeMap<String, SourceConnector>,
    pub(super) candidate_bindings: BTreeMap<String, SourceCandidateBinding>,
    pub(super) recovery_delays_ms: BTreeMap<String, u64>,
    pub(super) legacy_candidate_ids: BTreeMap<String, Option<String>>,
}

pub(super) struct AccountRuntimeParts {
    pub(super) executors: BTreeMap<String, ChatGptAccountExecutor>,
    pub(super) passive_quotas: BTreeMap<String, PassiveQuotaState>,
    pub(super) team_members: BTreeMap<String, BTreeSet<String>>,
}

struct ConfiguredKeyRule {
    enabled: bool,
    scope: CandidateScope,
    model_rules: ModelRules,
    client_wire_apis: Option<Vec<ClientWireApi>>,
}

pub(super) struct KeyRuntimeParts {
    pub(super) runtime_keys: Vec<RuntimeKey>,
    configured_rules: Vec<ConfiguredKeyRule>,
}

pub(super) fn validate_runtime_options(options: &GatewayRuntimeOptions) -> Result<()> {
    if let Some(policy) = &options.pool_routing {
        policy
            .validate_activation()
            .map_err(|message| Error::Validation(message.into()))?;
    }
    if !u8::try_from(options.max_retry_candidates)
        .is_ok_and(crate::protocol::max_retry_candidates_in_range)
    {
        return Err(Error::Validation(
            "max retry candidates must be between 1 and 8".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn configure_scheduler(options: &GatewayRuntimeOptions) -> Result<PoolScheduler> {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_quota_stale_after_ms(options.quota_stale_after_ms);
    Ok(scheduler)
}

pub(super) fn validate_reachability(
    requirement: ReachabilityRequirement,
    sources: &SourceRuntimeParts,
    accounts: &AccountRuntimeParts,
    keys: &KeyRuntimeParts,
    scheduler: &PoolScheduler,
) -> Result<()> {
    if !matches!(requirement, ReachabilityRequirement::RequireReachable) {
        return Ok(());
    }
    if sources.executors.is_empty() && accounts.executors.is_empty() {
        return Err(Error::Validation(
            "at least one provider source or OAuth account is required".to_string(),
        ));
    }
    if !keys.runtime_keys.iter().any(|key| key.enabled) {
        return Err(Error::Validation(
            "at least one enabled gateway credential is required".to_string(),
        ));
    }
    let has_usable_key = keys
        .configured_rules
        .iter()
        .filter(|rule| rule.enabled)
        .any(|rule| {
            let allowed_protocols = rule
                .client_wire_apis
                .as_deref()
                .map_or_else(all_native_wire_apis, client_wire_apis_to_native);
            scheduler.candidates().any(|candidate| {
                candidate.models.iter().any(|model| {
                    rule.model_rules.allows(model)
                        && candidate.is_configured(model, &allowed_protocols, &rule.scope)
                })
            })
        });
    if !has_usable_key {
        return Err(Error::Validation(
            "no enabled gateway credential can reach a configured source candidate".to_string(),
        ));
    }
    Ok(())
}

impl GatewayRuntime {
    pub fn new(
        source: ProviderSource,
        local_key: LocalGatewayKey,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::from_pool(
            vec![RuntimeSource::unrestricted(source)],
            vec![RuntimeLocalKey::unrestricted(local_key)],
            GatewayRuntimeOptions::default(),
            usage,
        )
    }

    pub fn from_pool(
        sources: Vec<RuntimeSource>,
        keys: Vec<RuntimeLocalKey>,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::build(
            sources,
            Vec::new(),
            keys.into_iter().map(Into::into).collect(),
            None,
            ReachabilityRequirement::RequireReachable,
            options,
            usage,
        )
    }

    pub fn from_mixed_pool(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeChatGptAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: RuntimeChatGptAuth,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::build(
            sources,
            accounts,
            keys,
            Some(account_auth),
            ReachabilityRequirement::RequireReachable,
            options,
            usage,
        )
    }

    /// Builds a locally managed gateway while its persisted configuration is
    /// temporarily unroutable.
    ///
    /// Desktop source and pool mutations can validly remove the final
    /// candidate. In that state the gateway must keep authenticating its
    /// gateway credentials and return an empty catalog instead of preventing the
    /// mutation from committing. Authentication, catalog visibility, and
    /// per-request candidate selection remain unchanged; callers validating a
    /// new configuration must use [`Self::from_mixed_pool`] instead.
    pub fn from_mixed_pool_allow_unroutable(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeChatGptAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: RuntimeChatGptAuth,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::build(
            sources,
            accounts,
            keys,
            Some(account_auth),
            ReachabilityRequirement::AllowUnroutable,
            options,
            usage,
        )
    }

    pub(super) fn build(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeChatGptAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: Option<RuntimeChatGptAuth>,
        reachability_requirement: ReachabilityRequirement,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        validate_runtime_options(&options)?;
        let model_reasoning_allowed_levels = normalize_model_reasoning_allowed_levels(
            options.model_reasoning_allowed_levels.clone(),
        )
        .map_err(|message| Error::Validation(message.to_string()))?;
        let model_context_policies =
            crate::normalize_model_context_policies(options.model_context_policies.clone())
                .map_err(|message| Error::Validation(message.to_string()))?;

        let clients = RuntimeHttpClients::new(None)?;
        let discovery_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        let mut scheduler = configure_scheduler(&options)?;
        let mut registry = ModelRegistry::default();
        // Validate legacy configuration on import; native image requests no
        // longer select a text model or use text pricing for route selection.
        normalize_image_base_model(options.image_base_model.clone())?;
        let reference_catalog = options
            .model_metadata_catalog
            .as_ref()
            .map(|handle| handle.snapshot());
        let source_parts = build_sources(
            sources,
            &mut registry,
            &mut scheduler,
            reference_catalog.as_deref(),
        )?;
        let account_parts = accounts::build_accounts(
            accounts,
            account_auth.as_ref(),
            options.native_codex_images_only,
            &source_parts,
            &mut registry,
            &mut scheduler,
        )?;
        let hidden_models = options
            .hidden_models
            .iter()
            .map(|model| crate::model_id_key(model))
            .filter(|model| !model.is_empty())
            .collect();
        // All callers use pool rotation. Older settings migrate once to the
        // same explicit policy used by desktop and server.
        let policy = options
            .pool_routing
            .clone()
            .unwrap_or_else(|| scheduler.migrated_pool_routing());
        scheduler.set_pool_routing(policy)?;
        let key_parts = build_keys(keys)?;
        validate_reachability(
            reachability_requirement,
            &source_parts,
            &account_parts,
            &key_parts,
            &scheduler,
        )?;

        let affinity_store = options.response_affinity_store.clone();
        let continuation_state = options.continuation_state.clone().unwrap_or_default();
        let response_route_affinity = continuation_state.routes.clone();
        if let Some(store) = affinity_store.as_ref() {
            let now_ms = runtime_now_ms();
            if let Ok(bindings) = store.load(now_ms) {
                for binding in bindings {
                    if binding.key.starts_with("route:") {
                        if binding.candidate_id.len() == 64
                            && binding
                                .candidate_id
                                .bytes()
                                .all(|byte| byte.is_ascii_hexdigit())
                        {
                            let mut routes = crate::poison::mutex(&response_route_affinity);
                            if routes.get_with_expiry(&binding.key, now_ms).is_none() {
                                routes.restore(
                                    binding.key,
                                    binding.candidate_id,
                                    binding.expires_at_ms,
                                    now_ms,
                                );
                            }
                        } else {
                            let _ = store.delete(&binding.key);
                        }
                        continue;
                    }
                    let soft =
                        binding.key.starts_with("cache:") || binding.key.starts_with("session:");
                    let owner = if scheduler.candidate(&binding.candidate_id).is_some() {
                        Some(binding.candidate_id.clone())
                    } else if soft {
                        source_parts
                            .legacy_candidate_ids
                            .get(&binding.candidate_id)
                            .cloned()
                            .flatten()
                    } else {
                        // Old suffixed opaque owners lack endpoint/credential
                        // identity. Never guess that they belong to a new route.
                        None
                    };
                    let Some(owner) = owner else {
                        let _ = store.delete(&binding.key);
                        continue;
                    };
                    let restored = if binding.key.starts_with("cache:")
                        || binding.key.starts_with("session:")
                    {
                        scheduler.restore_prompt_affinity(
                            binding.key.clone(),
                            &owner,
                            binding.expires_at_ms,
                            now_ms,
                        )
                    } else {
                        scheduler.restore_response_affinity(
                            binding.key.clone(),
                            &owner,
                            binding.expires_at_ms,
                            now_ms,
                        )
                    };
                    if !restored {
                        let _ = store.delete(&binding.key);
                    }
                }
            }
        }

        // Build per-source HTTP and WebSocket clients for sources that
        // configure a proxy. Sources without a proxy use the shared global
        // clients, so only proxied sources need dedicated clients here.
        let mut source_http_clients = BTreeMap::<String, reqwest::Client>::new();
        let mut source_websocket_clients = BTreeMap::<String, reqwest::Client>::new();
        for (candidate_id, binding) in &source_parts.candidate_bindings {
            let Some(connector) = source_parts.executors.get(&binding.source_id) else {
                continue;
            };
            if connector.proxy.is_some() {
                let proxy = connector.proxy.as_ref();
                source_http_clients.insert(
                    candidate_id.clone(),
                    crate::runtime::support::runtime_client(proxy)?,
                );
                source_websocket_clients.insert(
                    candidate_id.clone(),
                    crate::runtime::support::runtime_websocket_client(proxy)?,
                );
            }
        }

        Ok(Self {
            diagnostics_enabled: options.diagnostics_enabled,
            tool_policy: RwLock::new(options.tool_policy.clone()),
            clients,
            http3: super::http3::Http3Transport::new(),
            discovery_client,
            sources: source_parts.executors,
            source_http_clients,
            source_websocket_clients,
            source_candidate_bindings: source_parts.candidate_bindings,
            legacy_source_candidate_ids: source_parts.legacy_candidate_ids,
            response_route_affinity,
            source_recovery_delays_ms: Mutex::new(source_parts.recovery_delays_ms),
            chatgpt_accounts: account_parts.executors,
            chatgpt_team_members: account_parts.team_members,
            chatgpt_team_breaker_recent: Mutex::new(BTreeMap::new()),
            keys: key_parts.runtime_keys,
            model_state: super::models::RuntimeModelState::new(
                registry,
                hidden_models,
                model_reasoning_allowed_levels,
                model_context_policies,
            ),
            scheduler: Arc::new(Mutex::new(scheduler)),
            candidate_availability: Arc::new(tokio::sync::Notify::new()),
            admission: Mutex::default(),
            admission_changed: tokio::sync::Notify::new(),
            official_codex_ultra: Mutex::new(BTreeMap::new()),
            websocket_transport_observations: Mutex::new(BTreeMap::new()),
            model_metadata: SourceModelMetadataState::default(),
            model_metadata_catalog: options.model_metadata_catalog.clone(),
            passive_quotas: Mutex::new(account_parts.passive_quotas),
            adapter_continuation_store: continuation_state.adapters.clone(),
            agent_message_cipher: continuation_state.agent_messages.clone(),
            native_responses_replay_store: continuation_state.native_responses.clone(),
            cache_context_store: super::cache_context::CacheContextStore::default(),
            codex_turn_state_store: CodexTurnStateStore::default(),
            basis_points_catalog_store: super::tool_catalog::ToolCatalogStore::default(),
            control: RuntimeControl::default(),
            max_retry_candidates: std::sync::atomic::AtomicUsize::new(options.max_retry_candidates),
            quota_stale_after_ms: options.quota_stale_after_ms,
            default_service_tier_value: AtomicU8::new(options.default_service_tier.atomic_value()),
            response_affinity_store: affinity_store,
            activity_callback: Arc::new(Mutex::new(Arc::new(|_| {}))),
            activity_runtime_id: NEXT_ACTIVITY_RUNTIME_ID.fetch_add(1, Ordering::Relaxed),
            activity_revision: Arc::new(AtomicU64::new(0)),
            chatgpt_team_breaker_callback: Arc::new(Mutex::new(Arc::new(|_| {}))),
            codex_catalog_hints: Mutex::new(BTreeMap::new()),
            codex_catalog_hint_callback: Mutex::new(Arc::new(|_, _| false)),
            usage,
        })
    }
}

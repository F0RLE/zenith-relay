use super::images::select_image_main_model_with_catalog;
use super::{
    all_native_wire_apis, client_wire_apis_to_native, model_rules, normalize_client_wire_api,
    normalize_prefix, normalized_responses_url, normalized_set, require_runtime_value,
    source_candidate_id, AccountModelInventory, ChatGptAccountExecutor, GatewayRuntimeOptions,
    PassiveQuotaState, RuntimeHttpClients, RuntimeKey, RuntimeSource, SourceCandidateBinding,
    IMAGE_API_MODEL,
};
use crate::pricing::PricingCatalog;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{
    CodexIdentityEnvelope, RuntimeChatGptAccount, RuntimeChatGptAuth, BASIS_POINTS_RESPONSES_URL,
};
use crate::{
    CandidateHealth, CandidateKind, CandidateQuota, CandidateScope, Error, ModelRegistry,
    ModelRules, PoolScheduler, Result, RuntimeCandidate, RuntimeMixedLocalKey, SourceConnector,
    WireApi,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, RwLock};
#[derive(Clone, Copy)]
pub(super) enum ReachabilityRequirement {
    RequireReachable,
    AllowUnroutable,
}

pub(super) struct SourceRuntimeParts {
    pub(super) executors: BTreeMap<String, SourceConnector>,
    pub(super) candidate_bindings: BTreeMap<String, SourceCandidateBinding>,
    pub(super) recovery_delays_ms: BTreeMap<String, u64>,
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
    if !(1..=8).contains(&options.max_retry_candidates) {
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

pub(super) fn build_sources(
    sources: Vec<RuntimeSource>,
    registry: &mut ModelRegistry,
    scheduler: &mut PoolScheduler,
) -> Result<SourceRuntimeParts> {
    let mut executors = BTreeMap::new();
    let mut candidate_bindings = BTreeMap::new();
    let mut recovery_delays_ms = BTreeMap::new();
    for source in sources {
        source.source.validate()?;
        if source.weight == 0 {
            return Err(Error::Validation(
                "source weight must be at least one".to_string(),
            ));
        }
        if source.recovery_delay_seconds > 24 * 60 * 60 {
            return Err(Error::Validation(
                "source recovery delay must not exceed 24 hours".to_string(),
            ));
        }
        if executors.contains_key(&source.source.id) {
            return Err(Error::Validation("source ids must be unique".to_string()));
        }
        let bindings = source.protocol_config.resolve(
            &source.source.base_url,
            &source.source.models,
            &source.protocol_bindings,
            source.source.wire_api,
        )?;
        let source_id = source.source.id.clone();
        let connector = SourceConnector::new(&source.source, &bindings)?;
        let rules = model_rules(&source.allowed_models, &source.excluded_models);
        for binding in &bindings {
            let models = normalized_set(binding.model_ids.iter());
            if models.is_empty() {
                continue;
            }
            let candidate_id = source_candidate_id(&source_id, binding, source.source.wire_api);
            if candidate_bindings.contains_key(&candidate_id) {
                return Err(Error::Validation(
                    "source protocol candidate ids must be unique".to_string(),
                ));
            }
            let candidate = RuntimeCandidate {
                id: candidate_id.clone(),
                kind: CandidateKind::ApiSource,
                source_id: source_id.clone(),
                account_id: None,
                protocol: binding.wire_api,
                enabled: source.enabled,
                draining: source.draining,
                priority: source.priority,
                weight: source.weight,
                models: models.clone(),
                model_rules: rules.clone(),
                health: CandidateHealth::Healthy,
                quota: CandidateQuota::Unknown,
                provider_credits_micro_units: None,
                provider_credits_unlimited: false,
                quota_updated_at_ms: None,
                quota_reset_at_ms: None,
                cooldowns: BTreeMap::new(),
                last_used_at: source.last_used_at_ms,

                secret_available: true,
            };
            registry.replace(candidate_id.clone(), source.source.models.iter());
            scheduler.upsert(candidate);
            scheduler.set_native_route(&candidate_id, binding.adapter.is_passthrough());
            if source.recovery_delay_seconds > 0 {
                recovery_delays_ms.insert(
                    candidate_id.clone(),
                    source.recovery_delay_seconds.saturating_mul(1_000),
                );
            }
            candidate_bindings.insert(
                candidate_id,
                SourceCandidateBinding {
                    source_id: source_id.clone(),
                    binding_key: binding.key(),
                    wire_api: binding.wire_api,
                    adapter: binding.adapter,
                    reasoning_mode: binding.reasoning_mode,
                    cache_write_ttl: binding.cache_write_ttl,
                },
            );
        }
        executors.insert(source_id, connector);
    }
    Ok(SourceRuntimeParts {
        executors,
        candidate_bindings,
        recovery_delays_ms,
    })
}

pub(super) fn build_accounts(
    accounts: Vec<RuntimeChatGptAccount>,
    account_auth: Option<&RuntimeChatGptAuth>,
    image_base_model: Option<&str>,
    image_pricing_catalog: Option<&PricingCatalog>,
    sources: &SourceRuntimeParts,
    registry: &mut ModelRegistry,
    scheduler: &mut PoolScheduler,
) -> Result<AccountRuntimeParts> {
    if !accounts.is_empty() && account_auth.is_none() {
        return Err(Error::Validation(
            "OAuth accounts require token authority adapters".to_string(),
        ));
    }
    let mut executors = BTreeMap::new();
    let mut passive_quotas = BTreeMap::new();
    let mut team_members = BTreeMap::<String, BTreeSet<String>>::new();
    for account in accounts {
        require_runtime_value("account candidate id", &account.id)?;
        require_runtime_value("account source id", &account.source_id)?;
        require_runtime_value("ChatGPT account id", &account.chatgpt_account_id)?;
        if account.weight == 0 {
            return Err(Error::Validation(
                "account weight must be at least one".to_string(),
            ));
        }
        if sources.executors.contains_key(&account.id)
            || sources.candidate_bindings.contains_key(&account.id)
            || executors.contains_key(&account.id)
        {
            return Err(Error::Validation(
                "runtime candidate ids must be unique".to_string(),
            ));
        }
        let responses_url = normalized_responses_url(&account.responses_url)?;
        let basis_points_url = normalized_responses_url(BASIS_POINTS_RESPONSES_URL)?;
        passive_quotas.insert(
            account.id.clone(),
            PassiveQuotaState {
                last_persist_hint_ms: account.quota_snapshot.updated_at_ms.unwrap_or_default(),
                snapshot: account.quota_snapshot.clone(),
                dirty: false,
                force_persist: false,
            },
        );
        // OAuth identities must not share an HTTP/2 connection pool. A connection-level
        // failure for one account would otherwise abort concurrent streams on other accounts.
        let clients = RuntimeHttpClients::new(account.proxy.as_ref())?;
        let identity = CodexIdentityEnvelope::standard(&account.chatgpt_account_id)
            .map_err(|message| Error::Validation(message.to_string()))?;
        let mut published_models = account.models.clone();
        let models = normalized_set(account.models.iter());
        let image_main_model =
            select_image_main_model_with_catalog(&models, image_base_model, image_pricing_catalog);
        let mut candidate_models = models.clone();
        if image_main_model.is_some() {
            candidate_models.insert(IMAGE_API_MODEL.to_string());
            published_models.push(IMAGE_API_MODEL.to_string());
        }
        let candidate = RuntimeCandidate {
            id: account.id.clone(),
            kind: CandidateKind::OAuthAccount,
            source_id: account.source_id.clone(),
            account_id: Some(account.id.clone()),
            protocol: WireApi::Responses,
            enabled: account.enabled,
            draining: account.draining,
            priority: account.priority,
            weight: account.weight,
            models: candidate_models,
            model_rules: model_rules(&account.allowed_models, &account.excluded_models),
            health: account.health,
            quota: account.quota,
            provider_credits_micro_units: account.quota_snapshot.available_credits_micro_units,
            provider_credits_unlimited: account.quota_snapshot.provider_credits_unlimited,
            quota_updated_at_ms: account.quota_updated_at_ms,
            quota_reset_at_ms: account.quota_snapshot.limiting_reset_at_ms(),
            cooldowns: BTreeMap::new(),
            last_used_at: account.last_used_at_ms,

            secret_available: true,
        };
        let auth = account_auth.ok_or_else(|| {
            Error::Validation("OAuth accounts require token authority adapters".to_string())
        })?;
        registry.replace(candidate.id.clone(), published_models.iter());
        let candidate_id = candidate.id.clone();
        scheduler.upsert(candidate);
        team_members
            .entry(account.chatgpt_account_id.trim().to_ascii_lowercase())
            .or_default()
            .insert(candidate_id.clone());
        executors.insert(
            account.id.clone(),
            ChatGptAccountExecutor {
                id: account.id,
                source_id: account.source_id,
                chatgpt_account_id: account.chatgpt_account_id,
                identity,
                responses_url,
                basis_points_url,
                basis_points_enabled: AtomicBool::new(account.basis_points_enabled),
                model_inventory: RwLock::new(AccountModelInventory {
                    configured_models: models,
                    image_main_model,
                }),
                image_bridge_revision: Arc::new(AtomicU64::new(0)),
                token_authority: auth.token_authority.clone(),
                refresh_adapter: auth.refresh_adapter.clone(),
                persistence_adapter: auth.persistence_adapter.clone(),
                refresh_skew_ms: auth.refresh_skew_ms,
                clients,
                active: AtomicBool::new(true),
                agent_identity: RwLock::new(auth.agent_identities.get(&candidate_id).cloned()),
                agent_identity_revision: AtomicU64::new(0),
                agent_task_lock: tokio::sync::Mutex::new(()),
                routing_cookies: super::routing_cookies::RoutingCookies::default(),
            },
        );
    }
    Ok(AccountRuntimeParts {
        executors,
        passive_quotas,
        team_members,
    })
}

pub(super) fn build_keys(
    keys: Vec<RuntimeMixedLocalKey>,
    hidden_models: &BTreeSet<String>,
) -> Result<KeyRuntimeParts> {
    let mut runtime_keys = Vec::new();
    let mut configured_rules = Vec::new();
    let mut key_ids = HashSet::new();
    for key in keys {
        key.key.validate()?;
        if !key_ids.insert(key.key.id.clone()) {
            return Err(Error::Validation(
                "gateway credential ids must be unique".to_string(),
            ));
        }
        let scope = CandidateScope {
            source_ids: key.source_ids.map(|ids| normalized_set(ids.iter())),
            account_ids: key.account_ids.map(|ids| normalized_set(ids.iter())),
            model_rules: ModelRules::default(),
        };
        let base_model_rules = ModelRules {
            allowed: normalized_set(key.allowed_models.iter()),
            excluded: normalized_set(key.excluded_models.iter()),
        };
        let mut model_rules = base_model_rules.clone();
        model_rules.excluded.extend(hidden_models.iter().cloned());
        let client_wire_apis = key.wire_apis.map(|values| {
            values
                .into_iter()
                .map(normalize_client_wire_api)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        });
        if client_wire_apis.as_ref().is_some_and(Vec::is_empty) {
            return Err(Error::Validation(
                "gateway credential protocol scope must not be empty".to_string(),
            ));
        }
        configured_rules.push(ConfiguredKeyRule {
            enabled: key.enabled,
            scope: scope.clone(),
            model_rules: base_model_rules,
            client_wire_apis: client_wire_apis.clone(),
        });
        runtime_keys.push(RuntimeKey {
            id: key.key.id,
            enabled: key.enabled,
            secret_hash: Sha256::digest(key.key.secret.as_bytes()).into(),
            scope: Arc::new(RwLock::new(scope)),
            scope_revision: Arc::new(AtomicU64::new(0)),
            model_rules,
            model_prefix: normalize_prefix(key.model_prefix),
            client_wire_apis,
        });
    }
    Ok(KeyRuntimeParts {
        runtime_keys,
        configured_rules,
    })
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

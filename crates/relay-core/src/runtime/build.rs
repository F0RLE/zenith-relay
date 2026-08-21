use super::images::select_image_main_model;
use super::{
    all_native_wire_apis, client_wire_apis_to_native, model_rules, normalize_client_wire_api,
    normalize_prefix, normalized_responses_url, normalized_set, require_runtime_value,
    source_candidate_id, ChatGptAccountExecutor, GatewayRuntimeOptions, PassiveQuotaState,
    RuntimeHttpClients, RuntimeKey, RuntimeSource, SourceCandidateBinding, IMAGE_API_MODEL,
};
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{CodexIdentityEnvelope, RuntimeChatGptAccount, RuntimeChatGptAuth};
use crate::{
    normalize_subscription_plan_order, runtime_source_protocol_bindings, CandidateHealth,
    CandidateKind, CandidateQuota, CandidateScope, Error, ModelRegistry, ModelRules, PoolScheduler,
    Result, RuntimeCandidate, RuntimeMixedLocalKey, SourceConnector, WireApi,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
#[derive(Clone, Copy)]
pub(super) enum ReachabilityRequirement {
    RequireReachable,
    AllowUnroutable,
}

pub(super) struct SourceRuntimeParts {
    pub(super) executors: BTreeMap<String, SourceConnector>,
    pub(super) candidate_bindings: BTreeMap<String, SourceCandidateBinding>,
    pub(super) endpoint_domains: BTreeMap<String, String>,
    pub(super) recovery_delays_ms: BTreeMap<String, u64>,
}

pub(super) struct AccountRuntimeParts {
    pub(super) executors: BTreeMap<String, ChatGptAccountExecutor>,
    pub(super) passive_quotas: BTreeMap<String, PassiveQuotaState>,
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
    if !(1..=8).contains(&options.max_retry_candidates) {
        return Err(Error::Validation(
            "max retry candidates must be between 1 and 8".to_string(),
        ));
    }
    if !(1..=8).contains(&options.cooldown_after_failures) {
        return Err(Error::Validation(
            "cooldown after failures must be between 1 and 8".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn configure_scheduler(options: &GatewayRuntimeOptions) -> Result<PoolScheduler> {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_cooldown_policy(
        options.cooldown_after_failures,
        options.keep_last_candidate_available,
    );
    scheduler.set_routing_strategy(options.routing_strategy);
    let subscription_plan_order =
        normalize_subscription_plan_order(options.subscription_plan_order.clone())
            .map_err(|message| Error::Validation(message.to_string()))?;
    scheduler.set_subscription_plan_order(&subscription_plan_order);
    scheduler.set_quota_stale_after_ms(options.quota_stale_after_ms);
    scheduler.set_provider_storm_breaker_enabled(options.provider_storm_breaker);
    Ok(scheduler)
}

pub(super) fn build_sources(
    sources: Vec<RuntimeSource>,
    registry: &mut ModelRegistry,
    scheduler: &mut PoolScheduler,
) -> Result<SourceRuntimeParts> {
    let mut executors = BTreeMap::new();
    let mut candidate_bindings = BTreeMap::new();
    let mut endpoint_domains = BTreeMap::new();
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
        let bindings = runtime_source_protocol_bindings(
            source.protocol_bindings.clone(),
            source.source.wire_api,
            &source.source.models,
        )?;
        let source_endpoint_domain =
            crate::sources::normalized_base_url(&source.source.base_url)?.to_string();
        let source_id = source.source.id.clone();
        let connector = SourceConnector::new(&source.source, &bindings)?;
        let rules = model_rules(&source.allowed_models, &source.excluded_models);
        for binding in &bindings {
            let models = normalized_set(binding.model_ids.iter());
            if models.is_empty() {
                continue;
            }
            let candidate_id = source_candidate_id(&source_id, binding, bindings.len());
            if candidate_bindings.contains_key(&candidate_id) {
                return Err(Error::Validation(
                    "source protocol candidate ids must be unique".to_string(),
                ));
            }
            endpoint_domains.insert(candidate_id.clone(), source_endpoint_domain.clone());
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
                quota_updated_at_ms: None,
                quota_reset_at_ms: None,
                cooldowns: BTreeMap::new(),
                last_used_at: source.last_used_at_ms,
                consecutive_failures: 0,
                secret_available: true,
            };
            registry.replace(candidate_id.clone(), binding.model_ids.iter());
            scheduler.upsert(candidate);
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
        endpoint_domains,
        recovery_delays_ms,
    })
}

pub(super) fn build_accounts(
    accounts: Vec<RuntimeChatGptAccount>,
    account_auth: Option<&RuntimeChatGptAuth>,
    image_base_model: Option<&str>,
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
        let image_main_model = select_image_main_model(&models, image_base_model);
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
            quota_updated_at_ms: account.quota_updated_at_ms,
            quota_reset_at_ms: account.quota_snapshot.limiting_reset_at_ms(),
            cooldowns: BTreeMap::new(),
            last_used_at: account.last_used_at_ms,
            consecutive_failures: 0,
            secret_available: true,
        };
        let auth = account_auth.ok_or_else(|| {
            Error::Validation("OAuth accounts require token authority adapters".to_string())
        })?;
        registry.replace(candidate.id.clone(), published_models.iter());
        let candidate_id = candidate.id.clone();
        scheduler.upsert(candidate);
        scheduler
            .set_candidate_subscription_expiry(&candidate_id, account.subscription_expires_at_ms);
        scheduler.set_candidate_subscription_plan(
            &candidate_id,
            account.subscription_plan_type.as_deref(),
        );
        executors.insert(
            account.id.clone(),
            ChatGptAccountExecutor {
                id: account.id,
                source_id: account.source_id,
                identity,
                responses_url,
                configured_models: models,
                image_main_model,
                token_authority: auth.token_authority.clone(),
                refresh_adapter: auth.refresh_adapter.clone(),
                persistence_adapter: auth.persistence_adapter.clone(),
                refresh_skew_ms: auth.refresh_skew_ms,
                clients,
                active: AtomicBool::new(true),
                agent_identity: RwLock::new(auth.agent_identities.get(&candidate_id).cloned()),
                agent_task_lock: tokio::sync::Mutex::new(()),
            },
        );
    }
    Ok(AccountRuntimeParts {
        executors,
        passive_quotas,
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

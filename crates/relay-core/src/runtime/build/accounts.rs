use super::super::images::select_image_main_model_with_catalog;
use super::super::{
    model_rules, normalized_responses_url, normalized_set, require_runtime_value,
    AccountModelInventory, ChatGptAccountExecutor, PassiveQuotaState, RuntimeHttpClients,
    IMAGE_API_MODEL,
};
use super::{AccountRuntimeParts, SourceRuntimeParts};
use crate::pricing::PricingCatalog;
use crate::providers::chatgpt::{
    CodexIdentityEnvelope, OAuthClientKind, RuntimeChatGptAccount, RuntimeChatGptAuth,
    BASIS_POINTS_RESPONSES_URL,
};
use crate::{
    CandidateKind, Error, ModelRegistry, PoolScheduler, Result, RuntimeCandidate, WireApi,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, RwLock};

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
        let excel = account.oauth_client_kind == OAuthClientKind::ExcelBps;
        if excel && account_auth.is_some_and(|auth| auth.agent_identities.contains_key(&account.id))
        {
            return Err(Error::Validation(
                "Excel OAuth cannot use ChatGPT Agent Identity".into(),
            ));
        }
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
        let image_main_model = (!excel)
            .then(|| {
                select_image_main_model_with_catalog(
                    &models,
                    image_base_model,
                    image_pricing_catalog,
                )
            })
            .flatten();
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
                oauth_client_kind: account.oauth_client_kind,
                id: account.id,
                source_id: account.source_id,
                chatgpt_account_id: account.chatgpt_account_id,
                chatgpt_user_id: account.chatgpt_user_id,
                basis_points_headers: account.basis_points_headers,
                identity,
                responses_url,
                basis_points_url,
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
                basis_points_access: RwLock::new(None),
                basis_points_access_refresh: tokio::sync::Mutex::new(()),
                routing_cookies: super::super::routing_cookies::RoutingCookies::default(),
            },
        );
    }
    Ok(AccountRuntimeParts {
        executors,
        passive_quotas,
        team_members,
    })
}

use crate::state::{
    now_ms, AccountCredential, AppState, GatewayKeyRecord, ServerAccountRecord, SourceRecord,
    COMMON_PROXY_SECRET_REF,
};
use crate::token_refresh::ServerTokenPersistence;
use reqwest::{header::HeaderValue, redirect::Policy};
use std::{sync::Arc, time::Duration};
use zenith_relay_core::{
    accounts::TokenPersistenceAdapter,
    protocol::{
        account_candidate_enabled, account_operational_state, operational_status,
        AccountOperationalInput, AccountSummary, ProxyMode, SourceSummary,
    },
    quota::{quota_economics_summary_for_revision, quota_valuation_revision},
    ApiEquivalentSummary, DefaultServiceTier, LocalGatewayKey, ProviderSource, ProxyConfig,
    RuntimeChatGptAccount, RuntimeMixedLocalKey, RuntimeSource,
};

pub(crate) fn account_proxy_config(
    state: &AppState,
    record: &ServerAccountRecord,
    credential: &AccountCredential,
) -> Result<Option<ProxyConfig>, String> {
    if let Some(proxy_id) = record.proxy_id.as_deref() {
        return proxy_config_by_id(state, proxy_id).map(Some);
    }
    if record.bypass_common_proxy {
        if state.store.account_proxy_required()? {
            return Err("an account proxy is required; direct account traffic is blocked".into());
        }
        return Ok(None);
    }
    if let Some(value) = credential.proxy_url.as_deref() {
        return ProxyConfig::parse(value)
            .map(Some)
            .map_err(|_| "stored account proxy URL is invalid".to_string());
    }
    if !state.store.common_proxy_configured()? {
        if state.store.account_proxy_required()? {
            return Err("an account proxy is required; direct account traffic is blocked".into());
        }
        return Ok(None);
    }
    if let Some(proxy_id) = state.store.common_proxy_id()? {
        return proxy_config_by_id(state, &proxy_id).map(Some);
    }
    let value = state
        .vault
        .load(COMMON_PROXY_SECRET_REF)?
        .ok_or_else(|| "common account proxy is configured but unavailable".to_string())?;
    ProxyConfig::parse(&value)
        .map(Some)
        .map_err(|_| "stored common proxy URL is invalid".to_string())
}

pub(crate) async fn ensure_server_agent_identity_task(
    state: &Arc<AppState>,
    record: &ServerAccountRecord,
    credential: AccountCredential,
    expected_task_id: Option<&str>,
) -> Result<AccountCredential, String> {
    let agent = credential
        .agent_identity()?
        .ok_or_else(|| "Agent Identity credential is missing".to_string())?;
    if agent.task_id().is_some()
        && expected_task_id.is_none_or(|expected| agent.task_id() != Some(expected))
    {
        return Ok(credential);
    }
    let proxy = account_proxy_config(state, record, &credential)?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent("Zenith Relay Server");
    let client = match proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| "Agent Identity task client is unavailable".to_string())?;
    let new_task_id = agent
        .register_task(&client)
        .await
        .map_err(|error| format!("failed to register Agent Identity task: {error}"))?;
    ServerTokenPersistence {
        state: state.clone(),
    }
    .persist_agent_task_id(&record.id, agent.task_id(), &new_task_id)
    .await
    .map_err(|error| error.code)?;
    let secret = state
        .vault
        .load(&record.secret_ref)?
        .ok_or_else(|| "stored Agent Identity credential is unavailable".to_string())?;
    let updated = serde_json::from_str(&secret)
        .map_err(|_| "stored Agent Identity credential is invalid".to_string())?;
    state.rebuild_runtime().await?;
    Ok(updated)
}

pub(crate) async fn prepare_server_account_authorization(
    state: &Arc<AppState>,
    record: &ServerAccountRecord,
    credential: AccountCredential,
    expected_task_id: Option<&str>,
) -> Result<(AccountCredential, HeaderValue), String> {
    if credential.is_agent_identity() {
        match ensure_server_agent_identity_task(state, record, credential.clone(), expected_task_id)
            .await
        {
            Ok(credential) => {
                let authorization = credential.authorization(now_ms())?;
                return Ok((credential, authorization));
            }
            Err(error) if !credential.has_oauth() => return Err(error),
            Err(_) => {}
        }
    }
    let tokens = state.prepare_account_tokens(&record.id).await?;
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", tokens.access_token()))
        .map_err(|_| "stored account access token is invalid".to_string())?;
    authorization.set_sensitive(true);
    Ok((credential, authorization))
}

fn proxy_config_by_id(state: &AppState, proxy_id: &str) -> Result<ProxyConfig, String> {
    let record = state
        .store
        .proxy(proxy_id)?
        .ok_or_else(|| "stored proxy reference is missing".to_string())?;
    let value = state
        .vault
        .load(&record.secret_ref)?
        .ok_or_else(|| "stored proxy secret is missing".to_string())?;
    ProxyConfig::parse(&value).map_err(|_| "stored proxy URL is invalid".to_string())
}

pub(super) fn common_proxy_available(state: &AppState, configured: bool) -> bool {
    configured
        && state.store.common_proxy_id().ok().flatten().map_or_else(
            || {
                state
                    .vault
                    .load(COMMON_PROXY_SECRET_REF)
                    .ok()
                    .flatten()
                    .is_some_and(|value| ProxyConfig::parse(&value).is_ok())
            },
            |proxy_id| proxy_config_by_id(state, &proxy_id).is_ok(),
        )
}

pub(super) fn account_proxy_status(
    state: &AppState,
    record: &ServerAccountRecord,
    credential: &AccountCredential,
    common_configured: bool,
    common_available: bool,
    account_proxy_required: bool,
) -> (ProxyMode, bool) {
    if let Some(proxy_id) = record.proxy_id.as_deref() {
        return (
            ProxyMode::Account,
            proxy_config_by_id(state, proxy_id).is_ok(),
        );
    }
    if record.bypass_common_proxy {
        return (ProxyMode::Direct, !account_proxy_required);
    }
    if let Some(value) = credential.proxy_url.as_deref() {
        return (ProxyMode::Account, ProxyConfig::parse(value).is_ok());
    }
    if common_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, !account_proxy_required)
}

pub(super) fn runtime_source(record: SourceRecord, api_key: String) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: record.id,
            name: record.name,
            base_url: record.base_url,
            api_key,
            wire_api: record.wire_api,
            models: record.models,
        },
        protocol_bindings: record.protocol_bindings,
        enabled: record.enabled,
        draining: record.draining,
        priority: record.priority,
        weight: record.weight,
        recovery_delay_seconds: record.recovery_delay_seconds,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        last_used_at_ms: None,
    }
}

pub(super) fn runtime_account(
    record: ServerAccountRecord,
    credential: &AccountCredential,
    proxy: Option<ProxyConfig>,
    quota_stale_after_ms: u64,
) -> RuntimeChatGptAccount {
    let operational = account_operational_state(AccountOperationalInput {
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        secret_available: true,
        proxy_available: true,
        auth_state: record.auth_state,
        health: record.health,
        subscription: &record.subscription,
        quota: &record.quota,
        last_error_code: record.last_error_code.as_deref(),
        now_ms: now_ms(),
        quota_stale_after_ms,
    });
    RuntimeChatGptAccount {
        id: record.id,
        source_id: record.source_id,
        chatgpt_account_id: credential.chatgpt_account_id.clone(),
        responses_url: credential.responses_url.clone(),
        models: record.models,
        enabled: account_candidate_enabled(record.enabled, operational.routing_block_reason),
        draining: record.draining,
        priority: record.priority,
        weight: record.weight,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        health: operational.health,
        quota: operational.quota,
        quota_updated_at_ms: record.quota.updated_at_ms,
        quota_snapshot: record.quota.clone(),
        subscription_plan_type: record.subscription.plan_type.clone(),
        subscription_expires_at_ms: record.subscription.active_until_ms,
        last_used_at_ms: record.last_used_at_ms,
        cooldowns: record.cooldowns,
        consecutive_failures: record.consecutive_failures,
        proxy,
    }
}

pub(super) fn runtime_key(
    record: GatewayKeyRecord,
    secret: String,
    pool_source_ids: &[String],
    pool_account_ids: &[String],
) -> RuntimeMixedLocalKey {
    RuntimeMixedLocalKey {
        key: LocalGatewayKey {
            id: record.id,
            secret,
        },
        enabled: record.enabled,
        source_ids: Some(pool_source_ids.to_vec()),
        account_ids: Some(pool_account_ids.to_vec()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        wire_apis: Some(vec![zenith_relay_core::protocol::ClientWireApi::Responses]),
    }
}

pub(super) fn source_summary(
    record: &SourceRecord,
    secret_available: bool,
    runtime_available: Option<bool>,
    api_equivalent: ApiEquivalentSummary,
) -> SourceSummary {
    SourceSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        operational_status: operational_status(
            record.enabled,
            false,
            !record.draining && secret_available,
            runtime_available,
        ),
        base_url: record.base_url.clone(),
        wire_api: record.wire_api,
        protocol_bindings: record.protocol_bindings.clone(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        recovery_delay_seconds: record.recovery_delay_seconds,
        model_price_overrides: record.model_price_overrides.clone(),
        detected_model_prices: record.detected_model_prices.clone(),
        api_equivalent,
        secret_available,
        last_error_code: record.last_error_code.clone(),
    }
}

pub(super) fn account_summary(
    record: &ServerAccountRecord,
    secret_available: bool,
    proxy_mode: ProxyMode,
    proxy_available: bool,
    api_equivalent: ApiEquivalentSummary,
    default_service_tier: DefaultServiceTier,
    quota_stale_after_ms: u64,
) -> AccountSummary {
    let operational = account_operational_state(AccountOperationalInput {
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        secret_available,
        proxy_available,
        auth_state: record.auth_state,
        health: record.health,
        subscription: &record.subscription,
        quota: &record.quota,
        last_error_code: record.last_error_code.as_deref(),
        now_ms: now_ms(),
        quota_stale_after_ms,
    });
    AccountSummary {
        id: record.id.clone(),
        label: record.label.clone(),
        identity_hint: record.identity_hint.clone(),
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        operational_status: operational.status,
        auth_state: record.auth_state,
        health: format!("{:?}", record.health).to_ascii_lowercase(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        economics: quota_economics_summary_for_revision(
            &record.economics,
            &record.quota,
            default_service_tier,
            now_ms(),
            quota_stale_after_ms,
            quota_valuation_revision(),
        ),
        subscription: record.subscription.clone(),
        quota: record.quota.clone(),
        quota_refresh_status: zenith_relay_core::protocol::quota_refresh_status(
            record.auth_state,
            &record.quota,
            false,
        ),
        secret_available,
        remote_location: None,
        proxy_mode,
        proxy_available,
        proxy_id: record.proxy_id.clone(),
        routing_block_reason: operational.routing_block_reason,
        last_error_code: record.last_error_code.clone(),
    }
}

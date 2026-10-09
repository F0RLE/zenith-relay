use crate::state::{
    now_ms, AccountCredential, AppState, ServerAccountRecord, SourceRecord, COMMON_PROXY_SECRET_REF,
};
use crate::token_refresh::ServerTokenPersistence;
use reqwest::{header::HeaderValue, redirect::Policy};
use std::{sync::Arc, time::Duration};
use zenith_relay_core::{
    accounts::{TokenPersistenceAdapter, TokenSet},
    model_metadata::ModelMetadataCatalog,
    protocol::{
        account_operational_state, AccountOperationalInput, AccountSummary, ProxyMode,
        QuotaWindowUsage, SourceSummary,
    },
    ApiEquivalentSummary, ProxyConfig,
};

pub(super) use super::runtime_records::{runtime_account, runtime_key, runtime_source};

pub(crate) fn account_proxy_config(
    state: &AppState,
    account_record: &ServerAccountRecord,
    credential: &AccountCredential,
) -> Result<Option<ProxyConfig>, String> {
    if let Some(proxy_id) = account_record.proxy_id.as_deref() {
        return proxy_config_by_id(state, proxy_id).map(Some);
    }
    if account_record.bypass_common_proxy {
        if state.store.account_proxy_required()? {
            return Err("an account proxy is required; direct account traffic is blocked".into());
        }
        return Ok(None);
    }
    if let Some(proxy_url) = credential.proxy_url.as_deref() {
        return ProxyConfig::parse(proxy_url)
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
    let common_proxy_secret = state
        .vault
        .load(COMMON_PROXY_SECRET_REF)?
        .ok_or_else(|| "common account proxy is configured but unavailable".to_string())?;
    ProxyConfig::parse(&common_proxy_secret)
        .map(Some)
        .map_err(|_| "stored common proxy URL is invalid".to_string())
}

pub(crate) async fn ensure_server_agent_identity_task(
    state: &Arc<AppState>,
    account_record: &ServerAccountRecord,
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
    let proxy = account_proxy_config(state, account_record, &credential)?;
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
    ServerTokenPersistence::for_account(state.clone(), account_record)
        .persist_agent_task_id_for_identity(&account_record.id, &agent, &new_task_id)
        .await
        .map_err(|error| error.code)?;
    let secret = state
        .vault
        .load(&account_record.secret_ref)?
        .ok_or_else(|| "stored Agent Identity credential is unavailable".to_string())?;
    let updated_credential = serde_json::from_str(&secret)
        .map_err(|_| "stored Agent Identity credential is invalid".to_string())?;
    if state
        .store
        .account(&account_record.id)?
        .as_ref()
        .map(|stored_account| &stored_account.secret_ref)
        != Some(&account_record.secret_ref)
    {
        return Err("account login changed during Agent task registration".into());
    }
    state.rebuild_runtime().await?;
    Ok(updated_credential)
}

pub(crate) async fn prepare_server_account_authorization(
    state: &Arc<AppState>,
    account_record: &ServerAccountRecord,
    credential: AccountCredential,
    expected_task_id: Option<&str>,
) -> Result<(AccountCredential, HeaderValue, Option<TokenSet>), String> {
    if credential.is_agent_identity() {
        match ensure_server_agent_identity_task(
            state,
            account_record,
            credential.clone(),
            expected_task_id,
        )
        .await
        {
            Ok(credential) => {
                let authorization = credential.authorization(now_ms())?;
                return Ok((credential, authorization, None));
            }
            Err(error) if !credential.has_oauth() => return Err(error),
            Err(_) => {}
        }
    }
    let tokens = state.prepare_account_tokens(account_record).await?;
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", tokens.access_token()))
        .map_err(|_| "stored account access token is invalid".to_string())?;
    authorization.set_sensitive(true);
    Ok((credential, authorization, Some(tokens)))
}

fn proxy_config_by_id(state: &AppState, proxy_id: &str) -> Result<ProxyConfig, String> {
    let proxy_record = state
        .store
        .proxy(proxy_id)?
        .ok_or_else(|| "stored proxy reference is missing".to_string())?;
    let proxy_secret = state
        .vault
        .load(&proxy_record.secret_ref)?
        .ok_or_else(|| "stored proxy secret is missing".to_string())?;
    ProxyConfig::parse(&proxy_secret).map_err(|_| "stored proxy URL is invalid".to_string())
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
                    .is_some_and(|proxy_secret| ProxyConfig::parse(&proxy_secret).is_ok())
            },
            |proxy_id| proxy_config_by_id(state, &proxy_id).is_ok(),
        )
}

pub(super) fn account_proxy_status(
    state: &AppState,
    account_record: &ServerAccountRecord,
    credential: &AccountCredential,
    common_configured: bool,
    common_available: bool,
    account_proxy_required: bool,
) -> (ProxyMode, bool) {
    if let Some(proxy_id) = account_record.proxy_id.as_deref() {
        return (
            ProxyMode::Account,
            proxy_config_by_id(state, proxy_id).is_ok(),
        );
    }
    if account_record.bypass_common_proxy {
        return (ProxyMode::Direct, !account_proxy_required);
    }
    if let Some(proxy_url) = credential.proxy_url.as_deref() {
        return (ProxyMode::Account, ProxyConfig::parse(proxy_url).is_ok());
    }
    if common_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, !account_proxy_required)
}

pub(super) fn source_summary(
    source_record: &SourceRecord,
    secret_available: bool,
    runtime_available: Option<bool>,
    api_equivalent: ApiEquivalentSummary,
    reference_catalog: &ModelMetadataCatalog,
) -> SourceSummary {
    SourceSummary::from_stored_source(
        source_record,
        secret_available,
        runtime_available,
        api_equivalent,
        source_record.last_error_code.clone(),
        None,
        Some(reference_catalog),
    )
}

pub(super) struct AccountSummaryInputs {
    pub(super) oauth_client_kind: zenith_relay_core::providers::chatgpt::OAuthClientKind,
    pub(super) secret_available: bool,
    pub(super) basis_points_available: bool,
    pub(super) proxy_mode: ProxyMode,
    pub(super) proxy_available: bool,
    pub(super) api_equivalent: ApiEquivalentSummary,
    pub(super) quota_window_usage: Option<QuotaWindowUsage>,
    pub(super) quota_stale_after_ms: u64,
}

pub(super) fn account_summary(
    account_record: &ServerAccountRecord,
    inputs: AccountSummaryInputs,
) -> AccountSummary {
    let AccountSummaryInputs {
        oauth_client_kind,
        secret_available,
        basis_points_available,
        proxy_mode,
        proxy_available,
        api_equivalent,
        quota_window_usage,
        quota_stale_after_ms,
    } = inputs;
    let operational = account_operational_state(AccountOperationalInput::from_source(
        account_record,
        secret_available,
        proxy_available,
        now_ms(),
        quota_stale_after_ms,
    ));
    AccountSummary {
        credit_balance_key: None,
        id: account_record.id.clone(),
        label: account_record.label.clone(),
        identity_hint: account_record.identity_hint.clone(),
        provider_family: account_record.provider_family.clone(),
        oauth_client_kind,
        basis_points_available,
        basis_points_enabled: basis_points_available
            && oauth_client_kind
                == zenith_relay_core::providers::chatgpt::OAuthClientKind::ExcelBps,
        enabled: account_record.enabled,
        in_pool: account_record.in_pool,
        draining: account_record.draining,
        operational_status: operational.status,
        auth_state: account_record.auth_state,
        health: account_record.health.summary_label(),
        models: account_record.effective_models().to_vec(),
        allowed_models: account_record.allowed_models.clone(),
        excluded_models: account_record.excluded_models.clone(),
        priority: account_record.priority,
        weight: account_record.weight,
        api_equivalent,
        quota_window_usage,
        purchase_cost_micro_usd: account_record.purchase_cost_micro_usd,
        subscription: account_record.subscription.clone(),
        quota: account_record.quota.clone(),
        quota_refresh_status: zenith_relay_core::protocol::quota_refresh_status(
            account_record.auth_state,
            &account_record.quota,
            false,
        ),
        refresh_state: Default::default(),
        secret_available,
        remote_location: None,
        proxy_mode,
        proxy_available,
        proxy_id: account_record.proxy_id.clone(),
        routing_block_reason: operational.routing_block_reason,
        last_error_code: account_record.last_error_code.clone(),
        client_auth_status: None,
        last_client_login_redirect_at_ms: None,
    }
}

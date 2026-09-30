use super::import_orchestrator::{
    credential_local_error, imported_identity, ImportItemError, ImportItemStatus,
};
#[cfg(test)]
use super::refresh_observations::model_refresh_error_kind;
use super::refresh_observations::{
    apply_models_read, apply_quota_read, record_read_error, AccountRefreshScope, RefreshReadKind,
};
use crate::local_pool::accounts::authority::{
    CredentialPersistence, ProcessAccountLocks, ProcessLockConfig, ProcessLockError,
    StoredRefreshAdapter,
};
use crate::local_pool::accounts::credentials::{
    bearer_authorization, CredentialRefresh, CredentialStore, StoredCodexCredentials,
};
use crate::local_pool::accounts::oauth::CodexOAuthClient;
use crate::local_pool::accounts::proxy::effective_proxy_config;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::{
    current_time_ms, sync_account_state_if_running, sync_refreshed_account_or_rollback,
};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{LocalAccountRecord, ProviderSourceRecord};
use crate::local_pool::profiles::codex;
use crate::local_pool::refresh::{self, RefreshRead};
use crate::local_pool::state::DesktopState;
use futures_util::{stream, StreamExt};
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use serde::Serialize;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, State};
use zenith_relay_core::accounts::{
    AccountAuthState, ReauthReason, TokenPersistenceAdapter, TokenRefreshFailureKind, TokenSet,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{
    is_agent_identity_task_invalid_failure, merge_subscription_metadata_at,
    subscription_refresh_due, AgentIdentityCredential, CodexModelsClient, CodexQuotaClient,
    CodexSubscriptionClient, CodexSubscriptionMetadata, ModelDiscoveryFailure,
    ModelDiscoveryFailureCode, QuotaRefreshOutcome,
};
use zenith_relay_core::quota::{QuotaRefreshFailure, QuotaTransition, Subscription};
use zenith_relay_core::scheduler::refresh::{http::ManagementHttpScope, RefreshKind};
use zenith_relay_core::ProxyConfig;

type CommandResult<T> = std::result::Result<T, CommandError>;

mod credentials;
mod profile;
mod quota;
mod reads;
mod types;

pub(in crate::local_pool::accounts) use credentials::reconcile_force_refreshed_account_record;
#[cfg(test)]
pub(in crate::local_pool::accounts) use credentials::{
    restore_force_refreshed_account_record, rollback_force_refreshed_before_authority,
};
pub(crate) use profile::sync_managed_account_profile;
pub(crate) use quota::{prepare_account_credentials, prepare_preserved_remote_account_credentials};
pub(in crate::local_pool::accounts) use quota::{
    refresh_account_quotas, refresh_manual_account_quota,
};
#[cfg(test)]
pub(in crate::local_pool::accounts) use reads::{
    apply_subscription_metadata, model_discovery_was_unauthorized, quota_refresh_was_unauthorized,
};
pub(in crate::local_pool::accounts) use reads::{
    ensure_local_agent_identity_task, mark_access_only_reauthentication,
    recover_account_authorization,
};
pub(in crate::local_pool) use reads::{
    prepare_account_request_authorization, read_account_models_once, read_account_quota_once,
};
pub(crate) use reads::{refresh_account_models_once, refresh_account_quota_once};
pub(in crate::local_pool) use types::PreparedAccountAuthorization;
pub(crate) use types::PreparedAccountCredentials;
pub use types::{
    AccountQuotaOutcome, AccountQuotaRefreshItemResult, AccountQuotaRefreshResponse,
    AccountQuotaRefreshStatus, ConfirmAccountImportResponse, CredentialRefreshResult,
    CredentialRefreshStatus, ImportItemResult,
};
pub(in crate::local_pool::accounts) use types::{
    QUOTA_COMMAND_TIMEOUT_OVERHEAD, QUOTA_REFRESH_BATCH_SIZE, TOKEN_REFRESH_SKEW_MS,
};

#[cfg(test)]
pub(in crate::local_pool::accounts) use credentials::{
    classify_manual_refresh_failure, is_credential_refresh_error_code,
};
pub(in crate::local_pool::accounts) use credentials::{
    persist_manual_refresh_failure, persisted_token_generation_is_newer, token_set_is_newer,
};
pub(in crate::local_pool::accounts) use types::MANAGED_PROFILE_OBSERVATION_LOCK;

struct AccountProjectionErrors {
    persist: &'static str,
    missing: &'static str,
    policy: &'static str,
}

async fn project_registered_account(
    state: &DesktopState,
    account_id: &str,
    tokens: &TokenSet,
    authoritative_tokens: &TokenSet,
    authoritative_auth_state: AccountAuthState,
    errors: AccountProjectionErrors,
) -> LocalResult<()> {
    let authority_state_changed = token_set_is_newer(authoritative_tokens, tokens)
        || authoritative_auth_state != AccountAuthState::Active;
    if authority_state_changed
        && reconcile_force_refreshed_account_record(
            state,
            account_id,
            tokens,
            authoritative_tokens,
            authoritative_auth_state,
        )
        .is_err()
    {
        return Err(
            crate::local_pool::commands::fail_closed(state, errors.persist.to_string()).await,
        );
    }
    let account = match state.store().and_then(|store| {
        store
            .account(account_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))
    }) {
        Ok(account) => account,
        Err(_) => {
            return Err(crate::local_pool::commands::fail_closed(
                state,
                errors.missing.to_string(),
            )
            .await);
        }
    };
    if !sync_account_state_if_running(state, &account.account.id).await {
        return Err(
            crate::local_pool::commands::fail_closed(state, errors.policy.to_string()).await,
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn refresh_local_account_quota(
    account_id: String,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<AccountQuotaRefreshResponse> {
    let quota = refresh_manual_account_quota(&state, &account_id).await;
    crate::local_pool::background::refresh_account_models_in_background(app, vec![account_id]);
    quota.map_err(Into::into)
}

/// Explicitly exercises the stored refresh token even when the current access
/// token is still usable. This is an account-health check requested by the
/// user, not a routing prerequisite.
#[tauri::command]
pub async fn force_refresh_local_account_credentials(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<CredentialRefreshResult> {
    let _mutation = state.setup_guard().await;
    credentials::force_refresh_account_credentials(&state, &account_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn refresh_all_local_account_quotas(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Vec<AccountQuotaRefreshItemResult>> {
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .filter(|account| account.remote_location.is_none())
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let results = refresh_account_quotas(&state, account_ids.clone()).await;
    crate::local_pool::background::refresh_account_models_in_background(app, account_ids);
    Ok(results)
}

fn load_stored_account<'a>(
    state: &'a DesktopState,
    account_id: &str,
) -> LocalResult<(
    std::sync::MutexGuard<'a, crate::local_pool::store::LocalPoolStore>,
    LocalAccountRecord,
)> {
    let store = state.store()?;
    let account = store
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    Ok((store, account))
}

#[cfg(test)]
mod tests;

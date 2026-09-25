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
use tauri::State;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialRefreshStatus {
    Refreshed,
    RetryableFailure,
    RequiresReauth,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRefreshResult {
    pub account_id: String,
    pub status: CredentialRefreshStatus,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

pub(super) const TOKEN_REFRESH_SKEW_MS: u64 = 60_000;

pub(super) const QUOTA_COMMAND_TIMEOUT_OVERHEAD: Duration = Duration::from_secs(5);

pub(super) const QUOTA_REFRESH_BATCH_SIZE: usize = 5;

// A profile observation is best effort. It must never hold a request or quota
// refresh behind an automatic credential rotation for the normal five-second
// mutation-lock timeout.
const MANAGED_PROFILE_OBSERVATION_LOCK: ProcessLockConfig = ProcessLockConfig {
    wait_timeout_ms: 125,
    poll_interval_ms: 25,
    stale_after_ms: 120_000,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum AccountQuotaOutcome {
    Skipped,
    Updated {
        transitions: Vec<QuotaTransition>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exhaustion_transitions: Vec<QuotaTransition>,
    },
    Failed {
        code: String,
        retryable: bool,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemResult {
    pub item_id: String,
    pub status: ImportItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<LocalAccountRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderSourceRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<AccountQuotaOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ImportItemError>,
}

impl ImportItemResult {
    pub(super) fn account_success(
        item_id: String,
        account: LocalAccountRecord,
        quota: AccountQuotaOutcome,
    ) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Succeeded,
            account: Some(account),
            source: None,
            quota: Some(quota),
            error: None,
        }
    }

    pub(super) fn source_success(item_id: String, source: ProviderSourceRecord) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Succeeded,
            account: None,
            source: Some(source),
            quota: None,
            error: None,
        }
    }

    pub(super) fn failure(item_id: String, error: ImportItemError) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Failed,
            account: None,
            source: None,
            quota: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmAccountImportResponse {
    pub session_id: String,
    pub results: Vec<ImportItemResult>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshResponse {
    pub account: LocalAccountRecord,
    pub quota: AccountQuotaOutcome,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exhaustion_transitions: Vec<QuotaTransition>,
}

pub(crate) struct PreparedAccountCredentials {
    pub(super) tokens: TokenSet,
    pub(super) provider_account_id: String,
    pub(super) proxy: Option<ProxyConfig>,
}

#[derive(Clone)]
pub(in crate::local_pool) struct PreparedAccountAuthorization {
    pub(super) authorization: HeaderValue,
    pub(super) subscription_authorization: Option<HeaderValue>,
    pub(super) tokens: Option<TokenSet>,
    pub(super) agent_task_id: Option<String>,
    pub(super) provider_account_id: String,
    pub(super) proxy: Option<ProxyConfig>,
}

impl fmt::Debug for PreparedAccountAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAccountAuthorization")
            .field("authorization", &"[redacted]")
            .field("subscription_authorization", &"[redacted]")
            .field("tokens", &"[redacted]")
            .field("agent_task_id", &"[redacted]")
            .field("provider_account_id", &"[redacted]")
            .field("proxy", &"[redacted]")
            .finish()
    }
}

impl PreparedAccountAuthorization {
    pub(super) fn from_tokens(value: PreparedAccountCredentials) -> LocalResult<Self> {
        let authorization = bearer_authorization(value.tokens.access_token()).map_err(|_| {
            LocalPoolError::new(ErrorCode::InvalidState, "account token is invalid")
        })?;
        Ok(Self {
            subscription_authorization: Some(authorization.clone()),
            authorization,
            tokens: Some(value.tokens),
            agent_task_id: None,
            provider_account_id: value.provider_account_id,
            proxy: value.proxy,
        })
    }
}

impl PreparedAccountCredentials {
    pub(crate) fn tokens(&self) -> &TokenSet {
        &self.tokens
    }

    pub(crate) fn provider_account_id(&self) -> &str {
        &self.provider_account_id
    }

    pub(crate) fn proxy(&self) -> Option<&ProxyConfig> {
        self.proxy.as_ref()
    }
}

impl fmt::Debug for PreparedAccountCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAccountCredentials")
            .field("tokens", &self.tokens)
            .field("provider_account_id", &"[redacted]")
            .field("proxy_configured", &self.proxy.is_some())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountQuotaRefreshStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshItemResult {
    pub account_id: String,
    pub status: AccountQuotaRefreshStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<AccountQuotaRefreshResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
}

#[tauri::command]
pub async fn refresh_local_account_quota(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<AccountQuotaRefreshResponse> {
    refresh_manual_account_quota(&state, &account_id)
        .await
        .map_err(Into::into)
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
    force_refresh_account_credentials(&state, &account_id)
        .await
        .map_err(Into::into)
}

async fn force_refresh_account_credentials(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<CredentialRefreshResult> {
    let account = state
        .store()?
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if account.remote_location.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "account is managed by a remote server",
        ));
    }
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "credential refresh lock is unavailable",
                )
            })?;
    let refresh_guard = locks.acquire(account_id).await.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::Conflict,
            "account credentials are being refreshed",
        )
    })?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let current = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    // Validate the identity needed to project refreshed credentials before any
    // local state changes. Previously this was checked after the secret,
    // authority, store, and runtime had already been updated.
    let provider_account_id = current
        .provider_account_id()
        .map(str::to_string)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account credentials do not contain a provider account id",
            )
        })?;
    let previous_tokens = current.to_token_set().map_err(credential_local_error)?;
    let Some(refresh_token) = current.refresh_token() else {
        drop(refresh_guard);
        persist_manual_refresh_failure(
            state,
            account_id,
            &current,
            ReauthReason::ExpiredRefreshToken,
            error_codes::REFRESH_TOKEN_MISSING,
        )
        .await?;
        return Ok(CredentialRefreshResult {
            account_id: account_id.to_string(),
            status: CredentialRefreshStatus::RequiresReauth,
            code: error_codes::REFRESH_TOKEN_MISSING.to_string(),
            expires_at_ms: current.expires_at_ms(),
            generation: Some(current.generation()),
        });
    };
    let settings = state.store()?.gateway().clone();
    let proxy = effective_proxy_config(&settings, &current)
        .map_err(|error| LocalPoolError::new(ErrorCode::GatewayUnavailable, error.message))?;
    let oauth = CodexOAuthClient::new_with_proxy(proxy.as_ref())
        .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "OAuth client is unavailable"))?;
    let now_ms = current_time_ms();
    let refreshed = match oauth.exchange_refresh_token(refresh_token, now_ms).await {
        Ok(tokens) => tokens,
        Err(failure) => {
            let (status, reason) = classify_manual_refresh_failure(failure.kind);
            drop(refresh_guard);
            if let Some(reason) = reason {
                persist_manual_refresh_failure(state, account_id, &current, reason, &failure.code)
                    .await?;
            }
            return Ok(CredentialRefreshResult {
                account_id: account_id.to_string(),
                status,
                code: failure.code,
                expires_at_ms: current.expires_at_ms(),
                generation: Some(current.generation()),
            });
        }
    };
    let updated = current
        .apply_refresh(
            CredentialRefresh::from_oauth(refreshed).map_err(credential_local_error)?,
            now_ms,
        )
        .map_err(credential_local_error)?;
    let tokens = updated.to_token_set().map_err(credential_local_error)?;
    let old_accounts = {
        let store = state.store()?;
        store.accounts().to_vec()
    };
    credentials.save(&updated).map_err(credential_local_error)?;
    // Keep the process lock while the durable account record and managed
    // profiles are updated. TokenAuthority can hold its account mutex while
    // its automatic adapter waits for this lock, so authority registration is
    // deliberately deferred until these reversible writes have succeeded.
    let account_write = (|| -> LocalResult<()> {
        let mut store = state.store()?;
        let mut updated_account = store
            .account(account_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        let was_reauth = matches!(
            updated_account.account.auth_state,
            AccountAuthState::RequiresReauth(_)
        );
        updated_account.account.auth_state = AccountAuthState::Active;
        updated_account.account.token_generation = updated.generation();
        updated_account.account.token_updated_at_ms = Some(now_ms);
        if was_reauth
            || updated_account
                .account
                .last_error_code
                .as_deref()
                .is_some_and(is_credential_refresh_error_code)
        {
            updated_account.account.last_error_code = None;
        }
        store.upsert_account(updated_account)
    })();
    if let Err(error) = account_write {
        return Err(rollback_force_refreshed_before_authority(
            state,
            &credentials,
            account_id,
            &current,
            &previous_tokens,
            &tokens,
            &old_accounts,
            &provider_account_id,
            false,
            error,
        )
        .await);
    }
    if let Err(error) =
        sync_account_profile_bindings(state, account_id, &tokens, &provider_account_id)
    {
        return Err(rollback_force_refreshed_before_authority(
            state,
            &credentials,
            account_id,
            &current,
            &previous_tokens,
            &tokens,
            &old_accounts,
            &provider_account_id,
            true,
            error,
        )
        .await);
    }
    // Releasing the process lock before touching TokenAuthority keeps the
    // global order acyclic. A late automatic refresh can only replace this
    // result with a newer generation, never be rolled back by it.
    drop(refresh_guard);
    let authority = state.token_authority();
    if let Err(error) = authority
        .register_if_newer(account_id, tokens.clone(), AccountAuthState::Active)
        .await
    {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            format!("failed to register refreshed credentials: {error}"),
        )
        .await);
    }
    let Some(authoritative_tokens) = authority.tokens(account_id).await else {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "refreshed account token state disappeared".to_string(),
        )
        .await);
    };
    let Some(authoritative_auth_state) = authority.auth_state(account_id).await else {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "refreshed account authentication state disappeared".to_string(),
        )
        .await);
    };
    let authority_state_changed = token_set_is_newer(&authoritative_tokens, &tokens)
        || authoritative_auth_state != AccountAuthState::Active;
    if authority_state_changed
        && reconcile_force_refreshed_account_record(
            state,
            account_id,
            &tokens,
            &authoritative_tokens,
            authoritative_auth_state,
        )
        .is_err()
    {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "newer refreshed account state could not be persisted".to_string(),
        )
        .await);
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
                "refreshed account state disappeared".to_string(),
            )
            .await);
        }
    };
    if !sync_account_state_if_running(state, &account.account.id).await {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "refreshed account state could not update the running account policy".to_string(),
        )
        .await);
    }
    // A successful token refresh does not prove that the official desktop
    // client left its login page. Keep the watchdog observation until the
    // client itself reports an available state; otherwise this action could
    // hide a real login redirect that happened while the refresh was running.
    Ok(CredentialRefreshResult {
        account_id: account_id.to_string(),
        status: CredentialRefreshStatus::Refreshed,
        code: "credentials_refreshed".to_string(),
        expires_at_ms: authoritative_tokens.expires_at_ms(),
        generation: Some(authoritative_tokens.generation()),
    })
}

fn sync_account_profile_bindings(
    state: &DesktopState,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> LocalResult<()> {
    codex::sync_account_bindings(
        &state.profile_backup_root(),
        account_id,
        tokens,
        provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        tokens,
        provider_account_id,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn rollback_force_refreshed_before_authority(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    previous_credentials: &StoredCodexCredentials,
    previous_tokens: &TokenSet,
    attempted_tokens: &TokenSet,
    old_accounts: &[LocalAccountRecord],
    provider_account_id: &str,
    profile_sync_started: bool,
    cause: LocalPoolError,
) -> LocalPoolError {
    // A profile sync can update more than one managed profile before detecting
    // an external change. Reapply the previous token generation when that
    // stage was entered, rather than leaving the desktop client out of sync
    // with Relay's stored credentials.
    let profiles_restored = !profile_sync_started
        || sync_account_profile_bindings(state, account_id, previous_tokens, provider_account_id)
            .is_ok();
    let credentials_restored = credentials.save(previous_credentials).is_ok();
    let records_restored =
        restore_force_refreshed_account_record(state, account_id, attempted_tokens, old_accounts)
            .unwrap_or(false);
    if profiles_restored && credentials_restored && records_restored {
        cause
    } else {
        crate::local_pool::commands::fail_closed(
            state,
            "credential refresh rollback could not restore the previous local account state"
                .to_string(),
        )
        .await
    }
}

pub(super) fn restore_force_refreshed_account_record(
    state: &DesktopState,
    account_id: &str,
    attempted_tokens: &TokenSet,
    old_accounts: &[LocalAccountRecord],
) -> LocalResult<bool> {
    let previous = old_accounts
        .iter()
        .find(|account| account.account.id == account_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let mut store = state.store()?;
    let mut current = store
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    // Never restore an older snapshot over a fresh login/rotation. Other
    // account fields (quota, health, model discovery, and policies) are left
    // untouched so a concurrent monitor update also survives the rollback.
    if persisted_token_generation_is_newer(
        current.account.token_generation,
        current.account.token_updated_at_ms,
        attempted_tokens,
    ) || current.account.auth_state != AccountAuthState::Active
    {
        return Ok(false);
    }
    current.account.auth_state = previous.account.auth_state;
    current.account.token_generation = previous.account.token_generation;
    current.account.token_updated_at_ms = previous.account.token_updated_at_ms;
    current.account.last_error_code = previous.account.last_error_code.clone();
    store.upsert_account(current)?;
    Ok(true)
}

pub(super) fn reconcile_force_refreshed_account_record(
    state: &DesktopState,
    account_id: &str,
    expected_tokens: &TokenSet,
    authoritative_tokens: &TokenSet,
    authoritative_auth_state: AccountAuthState,
) -> LocalResult<()> {
    let mut store = state.store()?;
    let mut account = store
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if persisted_token_generation_is_newer(
        account.account.token_generation,
        account.account.token_updated_at_ms,
        expected_tokens,
    ) || account.account.auth_state != AccountAuthState::Active
    {
        return Ok(());
    }
    account.account.token_generation = authoritative_tokens.generation();
    account.account.token_updated_at_ms = Some(authoritative_tokens.issued_at_ms());
    account.account.auth_state = authoritative_auth_state;
    if authoritative_auth_state == AccountAuthState::Active
        && account
            .account
            .last_error_code
            .as_deref()
            .is_some_and(is_credential_refresh_error_code)
    {
        account.account.last_error_code = None;
    }
    store.upsert_account(account)
}

fn token_set_is_newer(candidate: &TokenSet, current: &TokenSet) -> bool {
    candidate.generation() > current.generation()
        || (candidate.generation() == current.generation()
            && candidate.issued_at_ms() > current.issued_at_ms())
}

fn is_credential_refresh_error_code(code: &str) -> bool {
    matches!(
        code,
        error_codes::INVALID_GRANT
            | error_codes::REFRESH_TOKEN_MISSING
            | error_codes::REFRESH_TOKEN_EXPIRED
            | error_codes::REFRESH_TOKEN_INVALIDATED
            | error_codes::TOKEN_INVALIDATED
    ) || code.starts_with("auth_")
}

fn classify_manual_refresh_failure(
    kind: TokenRefreshFailureKind,
) -> (CredentialRefreshStatus, Option<ReauthReason>) {
    match kind {
        TokenRefreshFailureKind::InvalidGrant => (
            CredentialRefreshStatus::RequiresReauth,
            Some(ReauthReason::InvalidGrant),
        ),
        TokenRefreshFailureKind::ExpiredRefreshToken => (
            CredentialRefreshStatus::RequiresReauth,
            Some(ReauthReason::ExpiredRefreshToken),
        ),
        TokenRefreshFailureKind::InvalidatedRefreshToken => (
            CredentialRefreshStatus::RequiresReauth,
            Some(ReauthReason::InvalidatedRefreshToken),
        ),
        TokenRefreshFailureKind::ReusedRefreshToken | TokenRefreshFailureKind::Transient => {
            (CredentialRefreshStatus::RetryableFailure, None)
        }
    }
}

async fn persist_manual_refresh_failure(
    state: &DesktopState,
    account_id: &str,
    current: &StoredCodexCredentials,
    reason: ReauthReason,
    code: &str,
) -> LocalResult<()> {
    let tokens = current.to_token_set().map_err(credential_local_error)?;
    let auth_state = AccountAuthState::RequiresReauth(reason);
    // A manual refresh releases the cross-process credential lock before it
    // can await TokenAuthority. Persist the terminal result first, but only
    // while the account record still describes the credential generation that
    // failed. A just-finished desktop login or automatic refresh must win.
    let Some(account) =
        persist_manual_refresh_failure_record(state, account_id, &tokens, auth_state, code)?
    else {
        return Ok(());
    };
    if !sync_account_state_if_running(state, &account.account.id).await {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "credential refresh failure could not update the running account policy".to_string(),
        )
        .await);
    }

    // This conditional registration comes after the durable state update so
    // a later store/runtime error cannot leave an in-memory reauth state that
    // was never recorded. It also refuses to replace a newer automatic or
    // desktop credential generation.
    let authority = state.token_authority();
    let applied = match authority
        .register_if_not_stale(account_id, tokens.clone(), auth_state)
        .await
    {
        Ok(applied) => applied,
        Err(error) => {
            return Err(crate::local_pool::commands::fail_closed(
                state,
                format!("failed to update credential state: {error}"),
            )
            .await);
        }
    };
    if applied {
        return Ok(());
    }

    let Some(authoritative_tokens) = authority.tokens(account_id).await else {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "newer account token state disappeared".to_string(),
        )
        .await);
    };
    let Some(authoritative_auth_state) = authority.auth_state(account_id).await else {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "newer account authentication state disappeared".to_string(),
        )
        .await);
    };
    let reconciled = match reconcile_manual_refresh_failure_record(
        state,
        account_id,
        &tokens,
        authoritative_tokens,
        authoritative_auth_state,
        auth_state,
        code,
    ) {
        Ok(reconciled) => reconciled,
        Err(_) => {
            return Err(crate::local_pool::commands::fail_closed(
                state,
                "newer credential state could not be persisted".to_string(),
            )
            .await);
        }
    };
    if let Some(account) = reconciled {
        if !sync_account_state_if_running(state, &account.account.id).await {
            return Err(crate::local_pool::commands::fail_closed(
                state,
                "newer credential state could not update the running account policy".to_string(),
            )
            .await);
        }
    }
    Ok(())
}

fn persist_manual_refresh_failure_record(
    state: &DesktopState,
    account_id: &str,
    expected_tokens: &TokenSet,
    auth_state: AccountAuthState,
    code: &str,
) -> LocalResult<Option<LocalAccountRecord>> {
    let mut store = state.store()?;
    let mut account = store
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if persisted_token_generation_is_newer(
        account.account.token_generation,
        account.account.token_updated_at_ms,
        expected_tokens,
    ) {
        return Ok(None);
    }
    account.account.auth_state = auth_state;
    account.account.last_error_code = Some(code.to_string());
    store.upsert_account(account.clone())?;
    Ok(Some(account))
}

fn reconcile_manual_refresh_failure_record(
    state: &DesktopState,
    account_id: &str,
    failed_tokens: &TokenSet,
    authoritative_tokens: TokenSet,
    authoritative_auth_state: AccountAuthState,
    failed_auth_state: AccountAuthState,
    failure_code: &str,
) -> LocalResult<Option<LocalAccountRecord>> {
    let mut store = state.store()?;
    let mut account = store
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    // Do not overwrite a later persisted observer/refresh result. The check
    // covers both components of the token version because an external client
    // can rotate a generation without changing its access-token expiry.
    if persisted_token_generation_is_newer(
        account.account.token_generation,
        account.account.token_updated_at_ms,
        failed_tokens,
    ) || account.account.auth_state != failed_auth_state
        || account.account.last_error_code.as_deref() != Some(failure_code)
    {
        return Ok(None);
    }
    account.account.token_generation = authoritative_tokens.generation();
    account.account.token_updated_at_ms = Some(authoritative_tokens.issued_at_ms());
    account.account.auth_state = authoritative_auth_state;
    if authoritative_auth_state == AccountAuthState::Active {
        account.account.last_error_code = None;
    }
    store.upsert_account(account.clone())?;
    Ok(Some(account))
}

fn persisted_token_generation_is_newer(
    persisted_generation: u64,
    persisted_updated_at_ms: Option<u64>,
    expected_tokens: &TokenSet,
) -> bool {
    persisted_generation > expected_tokens.generation()
        || (persisted_generation == expected_tokens.generation()
            && persisted_updated_at_ms
                .is_some_and(|updated_at_ms| updated_at_ms > expected_tokens.issued_at_ms()))
}

#[tauri::command]
pub async fn refresh_all_local_account_quotas(
    state: State<'_, DesktopState>,
) -> CommandResult<Vec<AccountQuotaRefreshItemResult>> {
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .filter(|account| account.remote_location.is_none())
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    Ok(refresh_account_quotas(&state, account_ids).await)
}

pub(super) async fn refresh_account_quotas(
    state: &DesktopState,
    account_ids: Vec<String>,
) -> Vec<AccountQuotaRefreshItemResult> {
    // Buffered owns the in-flight futures on the heap. An inline join of five
    // full account refreshes makes Tauri's generated command dispatcher exceed
    // the Windows UI thread's stack, even when invoking a different command.
    stream::iter(account_ids)
        .map(|account_id| refresh_account_quota_item(state, account_id))
        .buffered(QUOTA_REFRESH_BATCH_SIZE)
        .collect()
        .await
}

async fn refresh_account_quota_item(
    state: &DesktopState,
    account_id: String,
) -> AccountQuotaRefreshItemResult {
    let result = refresh_manual_account_quota(state, &account_id).await;
    match result {
        Ok(response) => AccountQuotaRefreshItemResult {
            account_id,
            status: AccountQuotaRefreshStatus::Succeeded,
            response: Some(response),
            error: None,
        },
        Err(error) => AccountQuotaRefreshItemResult {
            account_id,
            status: AccountQuotaRefreshStatus::Failed,
            response: None,
            error: Some(error.into()),
        },
    }
}

pub(super) async fn refresh_manual_account_quota(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<AccountQuotaRefreshResponse> {
    // Independent kind jobs; each joins its own automatic read. A models
    // failure is persisted separately and must not erase a successful quota.
    let (quota, _models) = tokio::join!(
        refresh_account_quota_once(state, account_id),
        refresh_account_models_once(state, account_id),
    );
    quota
}

pub(crate) async fn sync_managed_account_profile(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<bool> {
    // Desktop login, explicit refresh, and automatic refresh all rotate the
    // same credential set. Serialize the credential snapshot and persistence
    // with the automatic refresh adapter's cross-process lock.
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), MANAGED_PROFILE_OBSERVATION_LOCK)
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "managed ChatGPT profile lock is unavailable",
                )
            })?;
    let profile_sync_guard = match locks.acquire(account_id).await {
        Ok(guard) => guard,
        // Automatic refresh already owns the credential snapshot. Use its
        // authority state instead of failing a caller just because the
        // optional desktop-profile observation must wait.
        Err(ProcessLockError::Timeout) => return Ok(false),
        Err(_) => {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "managed ChatGPT profile lock is unavailable",
            ));
        }
    };
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let stored = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    let provider_account_id = stored
        .provider_account_id()
        .map(str::to_string)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account credentials do not contain a provider account id",
            )
        })?;
    let stored_tokens = stored.to_token_set().map_err(credential_local_error)?;
    let now_ms = current_time_ms();
    let Some(update) = codex::managed_account_token_update(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        &stored_tokens,
        &provider_account_id,
    )?
    else {
        return Ok(false);
    };
    // The desktop auth file may omit id_token in a partial rotation. Keep the
    // previously verified ID token rather than clearing the local binding.
    let id_token = update
        .id_token
        .or_else(|| stored.id_token().map(str::to_string));
    let identity = imported_identity(id_token.as_deref(), Some(&update.access_token));
    if identity
        .provider_account_id
        .as_deref()
        .is_some_and(|value| value != provider_account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed ChatGPT profile token belongs to another account",
        ));
    }
    let tokens = TokenSet::new(
        update.access_token,
        Some(update.refresh_token),
        id_token,
        identity.access_expires_at_ms,
        now_ms,
        stored.generation().saturating_add(1),
    )
    .map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "managed ChatGPT profile tokens are invalid",
        )
    })?;
    let account_before = state
        .store()?
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if persisted_token_generation_is_newer(
        account_before.account.token_generation,
        account_before.account.token_updated_at_ms,
        &tokens,
    ) {
        return Ok(false);
    }
    let updated = stored
        .with_token_set(&tokens)
        .map_err(credential_local_error)?;
    credentials.save(&updated).map_err(credential_local_error)?;
    let account_write = (|| -> LocalResult<()> {
        let mut store = state.store()?;
        let mut account = store
            .account(account_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        if persisted_token_generation_is_newer(
            account.account.token_generation,
            account.account.token_updated_at_ms,
            &tokens,
        ) {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "managed ChatGPT profile token generation changed during synchronization",
            ));
        }
        account.account.token_generation = tokens.generation();
        account.account.token_updated_at_ms = Some(tokens.issued_at_ms());
        account.account.auth_state = AccountAuthState::Active;
        store.upsert_account(account)
    })();
    if account_write.is_err() {
        drop(profile_sync_guard);
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "managed ChatGPT token synchronization could not persist account state".to_string(),
        )
        .await);
    }
    // TokenAuthority holds its own account mutex while its refresh adapter
    // waits on this process lock. Release the process lock before touching the
    // authority to keep the global lock order acyclic. A conditional register
    // prevents a just-finished automatic refresh from being rolled back.
    drop(profile_sync_guard);
    let authority = state.token_authority();
    if let Err(error) = authority
        .register_if_newer(account_id, tokens.clone(), AccountAuthState::Active)
        .await
    {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            format!("failed to register managed ChatGPT tokens: {error}"),
        )
        .await);
    }
    let Some(authoritative_tokens) = authority.tokens(account_id).await else {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "managed ChatGPT token state disappeared".to_string(),
        )
        .await);
    };
    let Some(authoritative_auth_state) = authority.auth_state(account_id).await else {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "managed ChatGPT authentication state disappeared".to_string(),
        )
        .await);
    };
    let authority_state_changed = token_set_is_newer(&authoritative_tokens, &tokens)
        || authoritative_auth_state != AccountAuthState::Active;
    if authority_state_changed
        && reconcile_force_refreshed_account_record(
            state,
            account_id,
            &tokens,
            &authoritative_tokens,
            authoritative_auth_state,
        )
        .is_err()
    {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "newer managed ChatGPT state could not be persisted".to_string(),
        )
        .await);
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
                "managed ChatGPT account state disappeared".to_string(),
            )
            .await);
        }
    };
    if !sync_account_state_if_running(state, &account.account.id).await {
        return Err(crate::local_pool::commands::fail_closed(
            state,
            "managed ChatGPT state could not update the running account policy".to_string(),
        )
        .await);
    }
    // Profile writes do not call TokenAuthority, so they can take the
    // cross-process lock after registration without forming a lock cycle.
    let _profile_sync_guard = match locks.acquire(account_id).await {
        Ok(guard) => guard,
        // The new durable tokens and authority state are already committed.
        // A concurrent automatic refresh will project its own generation when
        // it finishes, so defer this optional profile write rather than
        // turning a successful login observation into an operation failure.
        Err(ProcessLockError::Timeout) => return Ok(true),
        Err(_) => {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "managed ChatGPT profile lock is unavailable",
            ));
        }
    };
    codex::sync_account_bindings(
        &state.profile_backup_root(),
        account_id,
        &authoritative_tokens,
        &provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        &authoritative_tokens,
        &provider_account_id,
    )?;
    Ok(true)
}

pub(crate) async fn prepare_account_credentials(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<PreparedAccountCredentials> {
    prepare_account_credentials_with_remote_policy(state, account_id, false).await
}

pub(crate) async fn prepare_preserved_remote_account_credentials(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<PreparedAccountCredentials> {
    prepare_account_credentials_with_remote_policy(state, account_id, true).await
}

pub(super) async fn prepare_account_credentials_with_remote_policy(
    state: &DesktopState,
    account_id: &str,
    allow_remote_location: bool,
) -> LocalResult<PreparedAccountCredentials> {
    let remote_location = state
        .store()?
        .account(account_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?
        .remote_location
        .clone();
    if remote_location.is_some() && !allow_remote_location {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "account is managed by a remote server",
        ));
    }
    sync_managed_account_profile(state, account_id).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let initial_account = state
        .store()?
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let stored = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    let gateway = state.store()?.gateway().clone();
    let proxy = effective_proxy_config(&gateway, &stored)
        .map_err(|error| LocalPoolError::new(ErrorCode::GatewayUnavailable, error.message))?;
    let authority = state.token_authority();
    authority
        .register_if_newer(
            account_id,
            stored.to_token_set().map_err(credential_local_error)?,
            initial_account.account.auth_state,
        )
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to register account token state: {error}"),
            )
        })?;
    let oauth = Arc::new(
        CodexOAuthClient::new_with_proxy(proxy.as_ref()).map_err(LocalPoolError::invalid_state)?,
    );
    let refresh = StoredRefreshAdapter::new(
        state.transient_root(),
        credentials.clone(),
        oauth,
        TOKEN_REFRESH_SKEW_MS,
    )
    .map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "failed to initialize account refresh locks",
        )
    })?;
    let persistence = CredentialPersistence::new(
        credentials.clone(),
        state.account_metadata_sink(),
        state.transient_root(),
    );
    let now_ms = current_time_ms();
    let prepared = authority
        .prepare_and_persist(
            account_id,
            now_ms,
            TOKEN_REFRESH_SKEW_MS,
            &refresh,
            &persistence,
        )
        .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(zenith_relay_core::accounts::TokenAuthorityError::AccessTokenExpired) => {
            mark_access_only_reauthentication(state, account_id).await?;
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "account access token expired and cannot be refreshed",
            ));
        }
        Err(error) => {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to prepare account credentials: {error}"),
            ))
        }
    };
    let current_credentials = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    let provider_account_id = current_credentials
        .provider_account_id()
        .map(str::to_string)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account credentials do not contain a provider account id",
            )
        })?;
    let proxy = effective_proxy_config(&gateway, &current_credentials)
        .map_err(|error| LocalPoolError::new(ErrorCode::GatewayUnavailable, error.message))?;
    codex::sync_account_bindings(
        &state.profile_backup_root(),
        account_id,
        &prepared.tokens,
        &provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        &prepared.tokens,
        &provider_account_id,
    )?;
    Ok(PreparedAccountCredentials {
        tokens: prepared.tokens,
        provider_account_id,
        proxy,
    })
}

pub(crate) async fn refresh_account_quota_once(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<AccountQuotaRefreshResponse> {
    match refresh::request(state, account_id, RefreshKind::Quota).await? {
        RefreshRead::Quota(response) => Ok(*response),
        _ => Err(LocalPoolError::invalid_state(
            "unexpected quota refresh result",
        )),
    }
}

pub(in crate::local_pool) async fn read_account_quota_once(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    force_subscription_refresh: bool,
) -> LocalResult<AccountQuotaRefreshResponse> {
    let quota_lock = state.quota_account_lock(&scope.fence.account_id)?;
    let _quota_guard = quota_lock.lock().await;
    scope.validate(state)?;
    let result = read_account_quota(state, scope, force_subscription_refresh).await;
    if let Err(error) = &result {
        record_read_error(state, scope, RefreshReadKind::Quota, error).await;
    }
    result
}

async fn read_account_quota(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    force_subscription_refresh: bool,
) -> LocalResult<AccountQuotaRefreshResponse> {
    let account_id = &scope.fence.account_id;
    let mut prepared = refresh::request_authorization(state, &scope.fence).await?;
    scope.validate(state)?;
    let http_scope = scope.http_scope(state);
    let now_ms = current_time_ms();
    let request_timeout =
        Duration::from_secs(state.store()?.gateway().quota_request_timeout_seconds);
    let account_before_refresh = &scope.before;
    let mut subscription = account_before_refresh.account.subscription.clone();
    if subscription.active_until_ms.is_none() {
        if let Some(active_until_ms) = prepared.tokens.as_ref().and_then(|tokens| {
            imported_identity(tokens.id_token(), Some(tokens.access_token()))
                .subscription_active_until_ms
        }) {
            subscription = zenith_relay_core::quota::Subscription::normalize(
                zenith_relay_core::quota::SubscriptionInput {
                    plan_type: subscription.plan_type.clone(),
                    active_until_ms: Some(active_until_ms),
                    forbidden: false,
                    observed_at_ms: now_ms,
                },
            );
        }
    }
    let refresh_subscription = force_subscription_refresh
        || subscription_refresh_due(
            subscription.active_until_ms,
            subscription.updated_at_ms,
            now_ms,
        );
    if refresh_subscription {
        let _subscription_guard = state.subscription_refresh_guard().await;
        scope.validate(state)?;
        if let Some(metadata) =
            request_subscription_metadata(&prepared, request_timeout, now_ms, &http_scope).await
        {
            apply_subscription_metadata(&mut subscription, metadata, now_ms);
        }
    }
    scope.validate(state)?;
    let mut refreshed = request_account_quota_metadata(
        &prepared,
        request_timeout,
        now_ms,
        &subscription,
        refresh_subscription,
        &http_scope,
    )
    .await?;
    respect_quota_retry_after(state, scope, &refreshed);
    scope.validate(state)?;
    if prepared.tokens.is_some() && quota_refresh_was_unauthorized(&refreshed) {
        match recover_account_authorization(
            state,
            account_id,
            prepared.tokens.as_ref().map(TokenSet::generation),
            current_time_ms(),
        )
        .await
        {
            Ok(recovered) => {
                prepared = PreparedAccountAuthorization::from_tokens(recovered)?;
                scope.validate(state)?;
                refreshed = request_account_quota_metadata(
                    &prepared,
                    request_timeout,
                    current_time_ms(),
                    &subscription,
                    refresh_subscription,
                    &http_scope,
                )
                .await?;
                respect_quota_retry_after(state, scope, &refreshed);
            }
            Err(_) if account_auth_is_access_only(state, account_id)? => {
                mark_access_only_reauthentication(state, account_id).await?;
            }
            Err(_) if !account_requires_reauthentication(state, account_id)? => {
                refreshed = Ok(QuotaRefreshOutcome::Failed {
                    failure: QuotaRefreshFailure::new(error_codes::QUOTA_TOKEN_REFRESH, true),
                    subscription: subscription.clone(),
                });
            }
            Err(_) => {}
        }
    } else if let Some(task_id) = prepared.agent_task_id.as_deref() {
        let invalid_task = quota_refresh_has_invalid_agent_task(&refreshed);
        if invalid_task {
            let stored = CredentialStore::from_backend(NativeSecretBackend)
                .require(account_id)
                .map_err(credential_local_error)?;
            prepared = match ensure_local_agent_identity_task(
                state,
                account_id,
                stored.clone(),
                Some(task_id),
            )
            .await
            {
                Ok(_) => prepare_account_request_authorization(state, account_id).await?,
                Err(_) if stored.has_oauth() => PreparedAccountAuthorization::from_tokens(
                    prepare_account_credentials(state, account_id).await?,
                )?,
                Err(error) => return Err(error),
            };
            scope.validate(state)?;
            refreshed = request_account_quota_metadata(
                &prepared,
                request_timeout,
                current_time_ms(),
                &subscription,
                refresh_subscription,
                &http_scope,
            )
            .await?;
            respect_quota_retry_after(state, scope, &refreshed);
        }
    }

    let _mutation = state.setup_guard().await;
    let quota = refreshed.unwrap_or_else(|_| QuotaRefreshOutcome::Failed {
        failure: QuotaRefreshFailure::new(error_codes::QUOTA_TIMEOUT, true),
        subscription: subscription.clone(),
    });
    let applied = {
        let mut store = state.store()?;
        apply_quota_read(&mut store, scope, quota, subscription, None)?
    };
    if zenith_relay_core::quota::subscription_plan_changed(
        applied.previous.account.subscription.plan_type.as_deref(),
        applied.account.account.subscription.plan_type.as_deref(),
    ) {
        state
            .refresh
            .mark_dirty(&scope.fence.identity(), RefreshKind::Models);
    }
    sync_refreshed_account_or_rollback(
        state,
        applied.previous,
        applied.account.clone(),
        applied.value.models_changed,
    )
    .await?;
    Ok(AccountQuotaRefreshResponse {
        account: applied.account,
        quota: applied.value.outcome,
        exhaustion_transitions: applied.value.exhaustion_transitions,
    })
}

/// Request the independent model-kind job (not an inline quota subrequest).
pub(crate) async fn refresh_account_models_once(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<()> {
    refresh::request(state, account_id, RefreshKind::Models)
        .await
        .map(|_| ())
}

pub(in crate::local_pool) async fn read_account_models_once(
    state: &DesktopState,
    scope: &AccountRefreshScope,
) -> LocalResult<bool> {
    scope.validate(state)?;
    let result = read_account_models(state, scope).await;
    if let Err(error) = &result {
        record_read_error(state, scope, RefreshReadKind::Models, error).await;
    }
    result
}

async fn read_account_models(
    state: &DesktopState,
    scope: &AccountRefreshScope,
) -> LocalResult<bool> {
    let account_id = &scope.fence.account_id;
    let mut prepared = refresh::request_authorization(state, &scope.fence).await?;
    scope.validate(state)?;
    let http_scope = scope.http_scope(state);
    let mut discovered_models = discover_account_models(&prepared, &http_scope).await;
    respect_models_retry_after(state, scope, &discovered_models);
    scope.validate(state)?;

    if prepared.tokens.is_some()
        && model_discovery_was_unauthorized(&Some(discovered_models.clone()))
    {
        match recover_account_authorization(
            state,
            account_id,
            prepared.tokens.as_ref().map(TokenSet::generation),
            current_time_ms(),
        )
        .await
        {
            Ok(recovered) => {
                prepared = PreparedAccountAuthorization::from_tokens(recovered)?;
                scope.validate(state)?;
                discovered_models = discover_account_models(&prepared, &http_scope).await;
                respect_models_retry_after(state, scope, &discovered_models);
            }
            Err(_) if account_auth_is_access_only(state, account_id)? => {
                mark_access_only_reauthentication(state, account_id).await?;
            }
            Err(_) if !account_requires_reauthentication(state, account_id)? => {}
            Err(_) => {}
        }
    }

    if prepared.agent_task_id.is_some()
        && model_discovery_has_invalid_agent_task(&Some(discovered_models.clone()))
    {
        let stored = CredentialStore::from_backend(NativeSecretBackend)
            .require(account_id)
            .map_err(credential_local_error)?;
        prepared =
            match ensure_local_agent_identity_task(state, account_id, stored.clone(), None).await {
                Ok(_) => prepare_account_request_authorization(state, account_id).await?,
                Err(_) if stored.has_oauth() => PreparedAccountAuthorization::from_tokens(
                    prepare_account_credentials(state, account_id).await?,
                )?,
                Err(error) => return Err(error),
            };
        scope.validate(state)?;
        discovered_models = discover_account_models(&prepared, &http_scope).await;
        respect_models_retry_after(state, scope, &discovered_models);
    }

    let _mutation = state.setup_guard().await;
    let succeeded = discovered_models.is_ok();
    let applied = {
        let mut store = state.store()?;
        apply_models_read(&mut store, scope, discovered_models)?
    };
    sync_refreshed_account_or_rollback(state, applied.previous, applied.account, applied.value)
        .await?;
    Ok(succeeded)
}

pub(super) async fn request_subscription_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
    http_scope: &ManagementHttpScope,
) -> Option<CodexSubscriptionMetadata> {
    let authorization = prepared.subscription_authorization.clone()?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(request_timeout);
    let client = match prepared.proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .ok()?;
    CodexSubscriptionClient::new(client)
        .ok()?
        .with_http_scope(http_scope.clone())
        .fetch_authorized(authorization, &prepared.provider_account_id, now_ms)
        .await
        .ok()
}

pub(super) fn apply_subscription_metadata(
    subscription: &mut Subscription,
    metadata: CodexSubscriptionMetadata,
    observed_at_ms: u64,
) {
    let mut plan_type = subscription.plan_type.clone();
    let mut active_until_ms = subscription.active_until_ms;
    merge_subscription_metadata_at(
        &mut plan_type,
        &mut active_until_ms,
        metadata,
        Some(observed_at_ms),
    );
    *subscription = Subscription::normalize(zenith_relay_core::quota::SubscriptionInput {
        plan_type,
        active_until_ms,
        forbidden: false,
        observed_at_ms,
    });
}

async fn request_account_quota_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
    subscription: &Subscription,
    refresh_subscription: bool,
    http_scope: &ManagementHttpScope,
) -> LocalResult<std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>> {
    let quota =
        CodexQuotaClient::new_with_proxy_and_timeout(prepared.proxy.as_ref(), request_timeout)
            .map_err(|failure| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    format!("failed to initialize quota client: {}", failure.code),
                )
            })?
            .with_http_scope(http_scope.clone());
    Ok(tokio::time::timeout(
        request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
        quota.refresh_quota_with_subscription_authorization(
            prepared.authorization.clone(),
            prepared.subscription_authorization.clone(),
            &prepared.provider_account_id,
            now_ms,
            subscription,
            refresh_subscription,
        ),
    )
    .await)
}

fn respect_quota_retry_after(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    result: &std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
) {
    if let Ok(QuotaRefreshOutcome::Failed { failure, .. }) = result {
        if let Some(delay) = failure.retry_after_ms() {
            state
                .refresh
                .respect_retry_after(&scope.fence.identity(), RefreshKind::Quota, delay);
        }
    }
}

fn respect_models_retry_after(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    result: &std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) {
    if let Some(delay) = result
        .as_ref()
        .err()
        .and_then(|failure| failure.retry_after_ms)
    {
        state
            .refresh
            .respect_retry_after(&scope.fence.identity(), RefreshKind::Models, delay);
    }
}

pub(super) async fn discover_account_models(
    prepared: &PreparedAccountAuthorization,
    http_scope: &ManagementHttpScope,
) -> std::result::Result<Vec<String>, ModelDiscoveryFailure> {
    let client = CodexModelsClient::new_with_proxy(prepared.proxy.as_ref())?
        .with_http_scope(http_scope.clone());
    let client_version = zenith_relay_core::providers::chatgpt::configured_codex_client_version();
    client
        .discover_authorized(
            prepared.authorization.clone(),
            &prepared.provider_account_id,
            &client_version,
        )
        .await
}

pub(in crate::local_pool) async fn prepare_account_request_authorization(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<PreparedAccountAuthorization> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let mut stored = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    if !stored.is_agent_identity() {
        return PreparedAccountAuthorization::from_tokens(
            prepare_account_credentials(state, account_id).await?,
        );
    }
    let account = state
        .store()?
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if account.remote_location.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "account is managed by a remote server",
        ));
    }
    stored = match ensure_local_agent_identity_task(state, account_id, stored.clone(), None).await {
        Ok(stored) => stored,
        Err(_) if stored.has_oauth() => {
            return PreparedAccountAuthorization::from_tokens(
                prepare_account_credentials(state, account_id).await?,
            );
        }
        Err(error) => return Err(error),
    };
    let provider_account_id = stored
        .provider_account_id()
        .map(str::to_string)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account credentials do not contain a provider account id",
            )
        })?;
    let gateway = state.store()?.gateway().clone();
    let proxy = effective_proxy_config(&gateway, &stored)
        .map_err(|error| LocalPoolError::new(ErrorCode::GatewayUnavailable, error.message))?;
    let subscription_authorization = if stored.has_oauth() {
        let oauth = prepare_account_credentials(state, account_id).await?;
        Some(
            bearer_authorization(oauth.tokens().access_token()).map_err(|_| {
                LocalPoolError::new(ErrorCode::InvalidState, "account token is invalid")
            })?,
        )
    } else {
        None
    };
    Ok(PreparedAccountAuthorization {
        authorization: stored
            .authorization(current_time_ms())
            .map_err(credential_local_error)?,
        subscription_authorization,
        tokens: None,
        agent_task_id: stored
            .agent_identity()
            .and_then(AgentIdentityCredential::task_id)
            .map(str::to_string),
        provider_account_id,
        proxy,
    })
}

pub(super) async fn ensure_local_agent_identity_task(
    state: &DesktopState,
    account_id: &str,
    stored: StoredCodexCredentials,
    expected_task_id: Option<&str>,
) -> LocalResult<StoredCodexCredentials> {
    let agent = stored.agent_identity().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "Agent Identity credential is missing",
        )
    })?;
    if agent.task_id().is_some()
        && expected_task_id.is_none_or(|expected| agent.task_id() != Some(expected))
    {
        return Ok(stored);
    }
    let gateway = state.store()?.gateway().clone();
    let proxy = effective_proxy_config(&gateway, &stored)
        .map_err(|error| LocalPoolError::new(ErrorCode::GatewayUnavailable, error.message))?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent("Zenith Relay");
    let client = match proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "task client is unavailable"))?;
    let new_task_id = agent.register_task(&client).await.map_err(|error| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            format!("failed to register Agent Identity task: {error}"),
        )
    })?;
    let persistence = CredentialPersistence::new(
        CredentialStore::from_backend(NativeSecretBackend),
        state.account_metadata_sink(),
        state.transient_root(),
    );
    persistence
        .persist_agent_task_id_for_identity(account_id, agent, &new_task_id)
        .await
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.code))?;
    CredentialStore::from_backend(NativeSecretBackend)
        .require(account_id)
        .map_err(credential_local_error)
}

pub(super) fn quota_refresh_has_invalid_agent_task(
    result: &std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
) -> bool {
    matches!(
        result,
        Ok(QuotaRefreshOutcome::Failed { failure, .. })
            if is_agent_identity_task_invalid_failure(failure)
    )
}

pub(super) fn model_discovery_has_invalid_agent_task(
    result: &Option<std::result::Result<Vec<String>, ModelDiscoveryFailure>>,
) -> bool {
    matches!(
        result,
        Some(Err(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::AgentTaskInvalid,
            ..
        }))
    )
}

pub(super) fn quota_refresh_was_unauthorized(
    result: &std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
) -> bool {
    matches!(
        result,
        Ok(QuotaRefreshOutcome::Failed { failure, .. })
            if failure.http_status() == Some(401)
    )
}

pub(super) fn model_discovery_was_unauthorized(
    result: &Option<std::result::Result<Vec<String>, ModelDiscoveryFailure>>,
) -> bool {
    matches!(
        result,
        Some(Err(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::Unauthorized,
            ..
        }))
    )
}

pub(super) async fn recover_account_authorization(
    state: &DesktopState,
    account_id: &str,
    failed_generation: Option<u64>,
    now_ms: u64,
) -> LocalResult<PreparedAccountCredentials> {
    let generation = failed_generation
        .ok_or_else(|| LocalPoolError::invalid_state("rejected token generation is missing"))?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let persistence = CredentialPersistence::new(
        credentials,
        state.account_metadata_sink(),
        state.transient_root(),
    );
    state
        .token_authority()
        .invalidate_access_generation_and_persist(
            account_id,
            Some(generation),
            now_ms,
            &persistence,
        )
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to invalidate rejected account access: {error}"),
            )
        })?;
    prepare_account_credentials(state, account_id).await
}

pub(super) fn account_requires_reauthentication(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<bool> {
    let auth_state = state
        .store()?
        .account(account_id)
        .map(|account| account.account.auth_state)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    Ok(auth_state.requires_fresh_login())
}

pub(super) fn account_auth_is_access_only(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<bool> {
    let auth_state = state
        .store()?
        .account(account_id)
        .map(|account| account.account.auth_state)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    Ok(auth_state == AccountAuthState::DegradedAccessOnly)
}

pub(super) async fn mark_access_only_reauthentication(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<()> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let current = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    persist_manual_refresh_failure(
        state,
        account_id,
        &current,
        ReauthReason::AccessTokenExpired,
        "access_token_expired",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_refresh_distinguishes_reauth_from_retryable_failures() {
        assert_eq!(
            classify_manual_refresh_failure(TokenRefreshFailureKind::InvalidGrant),
            (
                CredentialRefreshStatus::RequiresReauth,
                Some(ReauthReason::InvalidGrant)
            )
        );
        assert_eq!(
            classify_manual_refresh_failure(TokenRefreshFailureKind::ExpiredRefreshToken),
            (
                CredentialRefreshStatus::RequiresReauth,
                Some(ReauthReason::ExpiredRefreshToken)
            )
        );
        assert_eq!(
            classify_manual_refresh_failure(TokenRefreshFailureKind::InvalidatedRefreshToken),
            (
                CredentialRefreshStatus::RequiresReauth,
                Some(ReauthReason::InvalidatedRefreshToken)
            )
        );
        assert_eq!(
            classify_manual_refresh_failure(TokenRefreshFailureKind::ReusedRefreshToken),
            (CredentialRefreshStatus::RetryableFailure, None)
        );
        assert_eq!(
            classify_manual_refresh_failure(TokenRefreshFailureKind::Transient),
            (CredentialRefreshStatus::RetryableFailure, None)
        );
    }

    #[test]
    fn successful_refresh_only_clears_credential_owned_error_codes() {
        assert!(is_credential_refresh_error_code("invalid_grant"));
        assert!(is_credential_refresh_error_code("auth_invalid_grant"));
        assert!(is_credential_refresh_error_code("token_invalidated"));
        assert!(!is_credential_refresh_error_code("models_unauthorized"));
        assert!(!is_credential_refresh_error_code("quota_exhausted"));
    }

    #[test]
    fn stale_manual_refresh_failure_never_wins_over_a_newer_token_snapshot() {
        let failed = TokenSet::new("access", Some("refresh".into()), None, None, 100, 7).unwrap();

        assert!(!persisted_token_generation_is_newer(7, Some(100), &failed));
        assert!(persisted_token_generation_is_newer(8, Some(1), &failed));
        assert!(persisted_token_generation_is_newer(7, Some(101), &failed));
        assert!(!persisted_token_generation_is_newer(6, Some(999), &failed));
    }

    #[test]
    fn model_refresh_preparation_errors_have_stable_codes() {
        assert_eq!(
            model_refresh_error_kind(ErrorCode::GatewayUnavailable),
            Some(("models_proxy_unavailable", true))
        );
        assert_eq!(
            model_refresh_error_kind(ErrorCode::SecretStoreUnavailable),
            Some(("models_secret_store", true))
        );
        assert_eq!(
            model_refresh_error_kind(ErrorCode::ProfileRestoreBlocked),
            Some(("models_profile_restore", false))
        );
        assert_eq!(model_refresh_error_kind(ErrorCode::NotFound), None);
    }

    #[test]
    fn authorization_debug_and_refresh_cache_never_expose_prepared_secrets() {
        let prepared = PreparedAccountAuthorization {
            authorization: bearer_authorization("synthetic-unique-access").unwrap(),
            subscription_authorization: None,
            tokens: None,
            agent_task_id: Some("synthetic-unique-task".into()),
            provider_account_id: "synthetic-unique-provider".into(),
            proxy: None,
        };
        let value = Ok(RefreshRead::Authorization(Box::new(prepared)));
        let debug = format!("{value:?}");
        for secret in [
            "synthetic-unique-access",
            "synthetic-unique-task",
            "synthetic-unique-provider",
        ] {
            assert!(!debug.contains(secret));
        }
        assert!(!refresh::cache_observation(&value));
    }
}

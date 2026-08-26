use super::import_orchestrator::{
    apply_model_discovery, apply_model_discovery_failure, credential_local_error,
    imported_identity, preserve_newer_account_state, ImportItemError, ImportItemStatus,
};
use crate::local_pool::accounts::authority::{CredentialPersistence, StoredRefreshAdapter};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::oauth::CodexOAuthClient;
use crate::local_pool::accounts::proxy::effective_proxy_config;
use crate::local_pool::accounts::quota_service::apply_quota_failure;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::{current_time_ms, sync_refreshed_account_or_rollback};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{LocalAccountRecord, ProviderSourceRecord};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use serde::Serialize;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use zenith_relay_core::accounts::{AccountAuthState, TokenPersistenceAdapter, TokenSet};
use zenith_relay_core::providers::chatgpt::{
    is_agent_identity_task_invalid_failure, merge_subscription_metadata_at,
    subscription_refresh_due, AgentIdentityCredential, CodexModelsClient, CodexQuotaClient,
    CodexSubscriptionClient, CodexSubscriptionMetadata, ModelDiscoveryFailure,
    ModelDiscoveryFailureCode, QuotaRefreshOutcome,
};
use zenith_relay_core::quota::{QuotaRefreshFailure, QuotaTransition, Subscription};
use zenith_relay_core::ProxyConfig;

type CommandResult<T> = std::result::Result<T, CommandError>;

pub(super) const TOKEN_REFRESH_SKEW_MS: u64 = 60_000;

pub(super) const QUOTA_COMMAND_TIMEOUT_OVERHEAD: Duration = Duration::from_secs(5);

pub(super) const QUOTA_REFRESH_BATCH_SIZE: usize = 5;

pub(super) const QUOTA_REFRESH_RETRY_MS: u64 = 60_000;

pub(super) const QUOTA_IDLE_REFRESH_MS: u64 = 15 * 60_000;

pub(super) const QUOTA_RESET_REFRESH_MIN_DELAY_MS: u64 = 5_000;

pub(super) const QUOTA_RESET_REFRESH_JITTER_MS: u64 = 10_000;

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
pub(super) struct PreparedAccountAuthorization {
    pub(super) authorization: HeaderValue,
    pub(super) subscription_authorization: Option<HeaderValue>,
    pub(super) tokens: Option<TokenSet>,
    pub(super) agent_task_id: Option<String>,
    pub(super) provider_account_id: String,
    pub(super) proxy: Option<ProxyConfig>,
}

impl PreparedAccountAuthorization {
    pub(super) fn from_tokens(value: PreparedAccountCredentials) -> LocalResult<Self> {
        let authorization = account_bearer_authorization(value.tokens.access_token())?;
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

pub(super) fn account_bearer_authorization(access_token: &str) -> LocalResult<HeaderValue> {
    let mut authorization = HeaderValue::from_str(&format!("Bearer {access_token}"))
        .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "account token is invalid"))?;
    authorization.set_sensitive(true);
    Ok(authorization)
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
    let mut results = Vec::with_capacity(account_ids.len());
    for chunk in account_ids.chunks(QUOTA_REFRESH_BATCH_SIZE) {
        let (first, second, third, fourth, fifth) = tokio::join!(
            refresh_batch_slot(state, chunk.first()),
            refresh_batch_slot(state, chunk.get(1)),
            refresh_batch_slot(state, chunk.get(2)),
            refresh_batch_slot(state, chunk.get(3)),
            refresh_batch_slot(state, chunk.get(4)),
        );
        results.extend([first, second, third, fourth, fifth].into_iter().flatten());
    }
    results
}

pub(super) async fn refresh_batch_slot(
    state: &DesktopState,
    account_id: Option<&String>,
) -> Option<AccountQuotaRefreshItemResult> {
    let account_id = account_id?.clone();
    let result = refresh_manual_account_quota(state, &account_id).await;
    Some(match result {
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
    })
}

pub(super) async fn refresh_manual_account_quota(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<AccountQuotaRefreshResponse> {
    match refresh_account_quota_once(state, account_id, true, true).await {
        Ok(response) => {
            settle_manual_quota_refresh(state, account_id, &response)?;
            Ok(response)
        }
        Err(error) => {
            let _ = record_quota_refresh_error(state, account_id, &error, current_time_ms());
            settle_manual_quota_error(state, account_id, &error)?;
            Err(error)
        }
    }
}

pub(crate) async fn sync_managed_account_profile(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<bool> {
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
    let persistence =
        CredentialPersistence::new(credentials.clone(), state.account_metadata_sink());
    let now_ms = current_time_ms();
    let Some(update) = codex::managed_account_token_update(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        stored.access_token(),
        &provider_account_id,
    )?
    else {
        return Ok(false);
    };
    let identity = imported_identity(update.id_token.as_deref(), Some(&update.access_token));
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
        update.id_token,
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
    persistence
        .persist(account_id, &tokens)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!("failed to persist managed ChatGPT tokens: {}", error.code),
            )
        })?;
    persistence
        .persist_auth_state(account_id, AccountAuthState::Active)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!(
                    "failed to restore managed ChatGPT auth state: {}",
                    error.code
                ),
            )
        })?;
    state
        .token_authority()
        .register(account_id, tokens.clone(), AccountAuthState::Active)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to register managed ChatGPT tokens: {error}"),
            )
        })?;
    codex::sync_account_bindings(
        &state.profile_backup_root(),
        account_id,
        &tokens,
        &provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        &tokens,
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
        .register(
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
        CodexOAuthClient::new_with_proxy(proxy.as_ref())
            .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?,
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
    let persistence =
        CredentialPersistence::new(credentials.clone(), state.account_metadata_sink());
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
            mark_access_only_reauthentication(state, account_id)?;
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
    force_subscription_refresh: bool,
    refresh_models: bool,
) -> LocalResult<AccountQuotaRefreshResponse> {
    let quota_lock = state.quota_account_lock(account_id)?;
    let _quota_guard = quota_lock.lock().await;
    let mut prepared = prepare_account_request_authorization(state, account_id).await?;
    let now_ms = current_time_ms();
    let request_timeout =
        Duration::from_secs(state.store()?.gateway().quota_request_timeout_seconds);
    let account_before_refresh = state
        .store()?
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
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
        if let Some(metadata) =
            request_subscription_metadata(&prepared, request_timeout, now_ms).await
        {
            apply_subscription_metadata(&mut subscription, metadata, now_ms);
        }
    }
    let (mut refreshed, mut discovered_models) = request_account_metadata(
        &prepared,
        request_timeout,
        now_ms,
        &subscription,
        refresh_subscription,
        refresh_models,
    )
    .await?;
    if prepared.tokens.is_some()
        && (quota_refresh_was_unauthorized(&refreshed)
            || model_discovery_was_unauthorized(&discovered_models))
    {
        match recover_account_authorization(state, account_id, current_time_ms()).await {
            Ok(recovered) => {
                prepared = PreparedAccountAuthorization::from_tokens(recovered)?;
                (refreshed, discovered_models) = request_account_metadata(
                    &prepared,
                    request_timeout,
                    current_time_ms(),
                    &subscription,
                    refresh_subscription,
                    refresh_models,
                )
                .await?;
            }
            Err(_) if account_auth_is_access_only(state, account_id)? => {
                mark_access_only_reauthentication(state, account_id)?;
            }
            Err(_) if !account_requires_reauthentication(state, account_id)? => {
                refreshed = Ok(QuotaRefreshOutcome::Failed {
                    failure: QuotaRefreshFailure::new("quota_token_refresh", true),
                    subscription: subscription.clone(),
                });
            }
            Err(_) => {}
        }
    } else if let Some(task_id) = prepared.agent_task_id.as_deref() {
        let invalid_task = quota_refresh_has_invalid_agent_task(&refreshed)
            || model_discovery_has_invalid_agent_task(&discovered_models);
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
            (refreshed, discovered_models) = request_account_metadata(
                &prepared,
                request_timeout,
                current_time_ms(),
                &subscription,
                refresh_subscription,
                refresh_models,
            )
            .await?;
        }
    }

    let observed_plan = match &refreshed {
        Ok(QuotaRefreshOutcome::Updated(data)) => data
            .quota
            .subscription
            .as_ref()
            .and_then(|subscription| subscription.plan_type.as_deref())
            .or(subscription.plan_type.as_deref()),
        Ok(QuotaRefreshOutcome::Failed { .. }) | Err(_) => subscription.plan_type.as_deref(),
    };
    if discovered_models.is_none()
        && zenith_relay_core::quota::subscription_plan_changed(
            account_before_refresh
                .account
                .subscription
                .plan_type
                .as_deref(),
            observed_plan,
        )
    {
        discovered_models = Some(discover_account_models(&prepared).await);
    }
    let _mutation = state.setup_guard().await;
    let (old_accounts, old_keys, account, outcome, exhaustion_transitions, models_changed) = {
        let mut store = state.store()?;
        let old_accounts = store.accounts().to_vec();
        let old_keys = store.keys().to_vec();
        let current_account = store
            .account(account_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        let mut account = current_account.clone();
        let (outcome, exhaustion_transitions) = match refreshed {
            Ok(outcome) => {
                let (outcome, exhaustion_transitions) =
                    super::import_orchestrator::apply_quota_outcome_with_transitions(
                        &mut account,
                        outcome,
                        now_ms,
                    );
                (outcome, exhaustion_transitions)
            }
            Err(_) => {
                let failure = QuotaRefreshFailure::new("quota_timeout", true);
                apply_quota_failure(&mut account, &failure, now_ms);
                (
                    AccountQuotaOutcome::Failed {
                        code: failure.code,
                        retryable: failure.retryable,
                    },
                    Vec::new(),
                )
            }
        };
        if current_account.account.subscription == account_before_refresh.account.subscription
            && subscription != account_before_refresh.account.subscription
        {
            // The dedicated subscription probe is authoritative even when the
            // quota endpoint returns an older or incomplete plan hint.
            account.account.subscription = subscription.clone();
        }
        let models_changed = discovered_models
            .map(|discovered_models| apply_model_discovery(&mut account, discovered_models))
            .unwrap_or(false);
        preserve_newer_account_state(&mut account, &account_before_refresh, &current_account);
        store.upsert_account(account.clone())?;
        (
            old_accounts,
            old_keys,
            account,
            outcome,
            exhaustion_transitions,
            models_changed,
        )
    };
    sync_refreshed_account_or_rollback(state, account_id, models_changed, old_accounts, old_keys)
        .await?;
    Ok(AccountQuotaRefreshResponse {
        account,
        quota: outcome,
        exhaustion_transitions,
    })
}

/// Refresh only the account's upstream model catalog.
///
/// Model discovery has its own lifecycle: it runs when an active background
/// session starts and every eight hours afterwards. Keeping it outside the
/// quota queue prevents a long quota window from delaying discovery and avoids
/// issuing a quota request merely because the model catalog is due.
pub(crate) async fn refresh_account_models_once(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<()> {
    let model_lock = state.quota_account_lock(account_id)?;
    let _model_guard = model_lock.lock().await;
    let mut prepared = prepare_account_request_authorization(state, account_id).await?;
    let mut discovered_models = discover_account_models(&prepared).await;

    if prepared.tokens.is_some()
        && model_discovery_was_unauthorized(&Some(discovered_models.clone()))
    {
        match recover_account_authorization(state, account_id, current_time_ms()).await {
            Ok(recovered) => {
                prepared = PreparedAccountAuthorization::from_tokens(recovered)?;
                discovered_models = discover_account_models(&prepared).await;
            }
            Err(_) if account_auth_is_access_only(state, account_id)? => {
                mark_access_only_reauthentication(state, account_id)?;
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
        discovered_models = discover_account_models(&prepared).await;
    }

    let _mutation = state.setup_guard().await;
    let (old_accounts, old_keys, models_changed) = {
        let mut store = state.store()?;
        let old_accounts = store.accounts().to_vec();
        let old_keys = store.keys().to_vec();
        let current_account = store
            .account(account_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        let mut account = current_account.clone();
        let models_changed = apply_model_discovery(&mut account, discovered_models);
        store.upsert_account(account.clone())?;
        (old_accounts, old_keys, models_changed)
    };
    sync_refreshed_account_or_rollback(state, account_id, models_changed, old_accounts, old_keys)
        .await?;
    Ok(())
}

pub(super) async fn request_subscription_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
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

pub(super) async fn request_account_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
    subscription: &Subscription,
    refresh_subscription: bool,
    refresh_models: bool,
) -> LocalResult<(
    std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
    Option<std::result::Result<Vec<String>, ModelDiscoveryFailure>>,
)> {
    let quota =
        CodexQuotaClient::new_with_proxy_and_timeout(prepared.proxy.as_ref(), request_timeout)
            .map_err(|failure| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    format!("failed to initialize quota client: {}", failure.code),
                )
            })?;
    let model_discovery = async {
        if !refresh_models {
            return None;
        }
        Some(discover_account_models(prepared).await)
    };
    Ok(tokio::join!(
        tokio::time::timeout(
            request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
            quota.refresh_quota_with_subscription_authorization(
                prepared.authorization.clone(),
                prepared.subscription_authorization.clone(),
                &prepared.provider_account_id,
                now_ms,
                subscription,
                refresh_subscription,
            ),
        ),
        model_discovery,
    ))
}

pub(super) async fn discover_account_models(
    prepared: &PreparedAccountAuthorization,
) -> std::result::Result<Vec<String>, ModelDiscoveryFailure> {
    let client = CodexModelsClient::new_with_proxy(prepared.proxy.as_ref())?;
    client
        .discover_authorized(
            prepared.authorization.clone(),
            &prepared.provider_account_id,
            zenith_relay_core::providers::chatgpt::CODEX_MODELS_CLIENT_VERSION,
        )
        .await
}

pub(super) async fn prepare_account_request_authorization(
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
        Some(account_bearer_authorization(oauth.tokens().access_token())?)
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
    );
    persistence
        .persist_agent_task_id(account_id, agent.task_id(), &new_task_id)
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
    now_ms: u64,
) -> LocalResult<PreparedAccountCredentials> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let persistence = CredentialPersistence::new(credentials, state.account_metadata_sink());
    state
        .token_authority()
        .invalidate_access_and_persist(account_id, now_ms, &persistence)
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
    Ok(matches!(auth_state, AccountAuthState::RequiresReauth(_)))
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

pub(super) fn mark_access_only_reauthentication(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<()> {
    let mut store = state.store()?;
    let mut account = store
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    account.account.auth_state = AccountAuthState::RequiresReauth(
        zenith_relay_core::accounts::ReauthReason::AccessTokenExpired,
    );
    store.upsert_account(account)
}

pub(super) fn settle_manual_quota_refresh(
    state: &DesktopState,
    account_id: &str,
    response: &AccountQuotaRefreshResponse,
) -> LocalResult<()> {
    state.remove_quota_refresh(account_id)?;
    if let Some(due_at_ms) = next_quota_refresh_at(response, current_time_ms()) {
        state.sync_account_quota_refresh(account_id, due_at_ms)?;
    }
    Ok(())
}

pub(super) fn settle_manual_quota_error(
    state: &DesktopState,
    account_id: &str,
    error: &LocalPoolError,
) -> LocalResult<()> {
    state.remove_quota_refresh(account_id)?;
    if !matches!(&error.code, ErrorCode::NotFound) {
        state.sync_account_quota_refresh(
            account_id,
            current_time_ms().saturating_add(QUOTA_REFRESH_RETRY_MS),
        )?;
    }
    Ok(())
}

pub(crate) fn record_quota_refresh_error(
    state: &DesktopState,
    account_id: &str,
    error: &LocalPoolError,
    observed_at_ms: u64,
) -> LocalResult<()> {
    if matches!(error.code, ErrorCode::NotFound) {
        return Ok(());
    }
    let code = match error.code {
        ErrorCode::SecretStoreUnavailable => "quota_secret_store",
        ErrorCode::GatewayUnavailable => "quota_proxy_unavailable",
        ErrorCode::Conflict => "quota_account_location",
        ErrorCode::Io | ErrorCode::RecoveryRequired => "quota_storage",
        ErrorCode::InvalidState
        | ErrorCode::SourceTestFailed
        | ErrorCode::ProfileRestoreBlocked
        | ErrorCode::UnsupportedSchema => "quota_prepare",
        ErrorCode::NotFound => return Ok(()),
    };
    let mut store = state.store()?;
    let Some(mut account) = store.account(account_id).cloned() else {
        return Ok(());
    };
    apply_quota_failure(
        &mut account,
        &QuotaRefreshFailure::new(code, true),
        observed_at_ms,
    );
    store.upsert_account(account)
}

/// Persist failures that happen before the model endpoint can be queried.
///
/// The quota worker already records its preparation failures, but model
/// discovery has an independent eight-hour lifecycle. Keeping this mapping
/// separate prevents a credential-store or proxy failure from disappearing in
/// the background worker while retaining the last successful catalog for
/// routing.
pub(crate) fn record_model_refresh_error(
    state: &DesktopState,
    account_id: &str,
    error: &LocalPoolError,
) -> LocalResult<()> {
    let Some((code, retryable)) = model_refresh_error_kind(error.code) else {
        return Ok(());
    };
    let mut store = state.store()?;
    let Some(mut account) = store.account(account_id).cloned() else {
        return Ok(());
    };
    // A token-expiry transition has a more actionable auth state than a
    // generic preparation error. Leave that state to the auth UI instead of
    // replacing it with `models_prepare`.
    if matches!(
        account.account.auth_state,
        AccountAuthState::RequiresReauth(_)
    ) && code == "models_prepare"
    {
        return Ok(());
    }
    apply_model_discovery_failure(&mut account, code, retryable);
    store.upsert_account(account)
}

fn model_refresh_error_kind(code: ErrorCode) -> Option<(&'static str, bool)> {
    Some(match code {
        ErrorCode::SecretStoreUnavailable => ("models_secret_store", true),
        ErrorCode::GatewayUnavailable => ("models_proxy_unavailable", true),
        ErrorCode::Conflict => ("models_account_location", false),
        ErrorCode::Io | ErrorCode::RecoveryRequired => ("models_storage", true),
        ErrorCode::InvalidState | ErrorCode::SourceTestFailed | ErrorCode::UnsupportedSchema => {
            ("models_prepare", true)
        }
        ErrorCode::ProfileRestoreBlocked => ("models_profile_restore", false),
        ErrorCode::NotFound => return None,
    })
}

pub(crate) fn next_quota_refresh_at(
    response: &AccountQuotaRefreshResponse,
    now_ms: u64,
) -> Option<u64> {
    let idle_due = now_ms.saturating_add(QUOTA_IDLE_REFRESH_MS);
    match &response.quota {
        AccountQuotaOutcome::Updated { .. } => {
            let reset_delay = quota_reset_refresh_delay(&response.account.account.id);
            let reset_due = response
                .account
                .account
                .quota
                .primary
                .iter()
                .chain(response.account.account.quota.secondary.iter())
                .filter_map(|window| window.reset_at_ms)
                .filter(|reset_at_ms| *reset_at_ms > now_ms)
                .map(|reset_at_ms| reset_at_ms.saturating_add(reset_delay))
                .min();
            Some(reset_due.map_or(idle_due, |due_at_ms| due_at_ms.min(idle_due)))
        }
        AccountQuotaOutcome::Failed { retryable, .. } => {
            if matches!(
                response.account.account.auth_state,
                AccountAuthState::RequiresReauth(_)
            ) {
                None
            } else if *retryable {
                Some(now_ms.saturating_add(QUOTA_REFRESH_RETRY_MS))
            } else {
                Some(idle_due)
            }
        }
        AccountQuotaOutcome::Skipped => Some(idle_due),
    }
}

pub(super) fn quota_reset_refresh_delay(account_id: &str) -> u64 {
    QUOTA_RESET_REFRESH_MIN_DELAY_MS.saturating_add(
        account_id.bytes().fold(0_u64, |hash, byte| {
            hash.wrapping_mul(16_777_619) ^ u64::from(byte)
        }) % QUOTA_RESET_REFRESH_JITTER_MS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

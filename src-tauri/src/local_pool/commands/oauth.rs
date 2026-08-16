use super::sync_accounts_or_rollback;
use crate::local_pool::{
    accounts::{
        credentials::{
            CredentialError, CredentialErrorCode, CredentialStore, StoredCodexCredentials,
        },
        import_session::SecretBackend,
        oauth::{
            CodexOAuthClient, OAuthError, OAuthTokenSet, CODEX_OAUTH_CLIENT_ID, CODEX_OAUTH_SCOPE,
        },
        oauth_flow::{
            OAuthFlowError, OAuthFlowErrorCode, OAuthFlowEventSink, OAuthFlowManager,
            OAuthFlowStart, OAuthFlowStatus,
        },
        proxy::{common_proxy_config, effective_proxy_config, ensure_account_proxy},
        quota_refresh::{next_quota_refresh_at, AccountQuotaOutcome, AccountQuotaRefreshResponse},
        quota_service::{apply_quota_failure, apply_quota_success},
        records::{self, new_account_record, CODEX_SOURCE_ID},
        NativeSecretBackend,
    },
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalAccountRecord, LocalGatewayKeyRecord},
    state::DesktopState,
};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use url::Url;
use uuid::Uuid;
use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountAuthState, AccountHealthState},
    providers::chatgpt::{
        AgentIdentityCredential, CodexModelsClient, CodexQuotaClient, ModelDiscoveryFailure,
        ModelDiscoveryFailureCode,
    },
    quota::QuotaRefreshFailure,
    ProxyConfig,
};

const AUTHORIZATION_ENDPOINT: &str = "https://auth.openai.com/oauth/authorize";
const CALLBACK_PATH: &str = "/auth/callback";
const COMPLETION_CHECKPOINT_VERSION: u32 = 1;
const MAX_COMPLETION_CHECKPOINT_BYTES: usize = 256 * 1024;

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Clone, Copy)]
struct InitialModelIssue {
    code: &'static str,
    retryable: bool,
    auth_error: bool,
    blocked: bool,
}

#[tauri::command]
pub async fn start_codex_oauth(
    app: AppHandle,
    open_browser: Option<bool>,
    state: State<'_, DesktopState>,
) -> CommandResult<OAuthFlowStart> {
    let _mutation = state.setup_guard().await;
    let settings = state.store()?.gateway().clone();
    let proxy = common_proxy_config(&settings)?;
    ensure_account_proxy(&settings, proxy.as_ref())?;
    let oauth = CodexOAuthClient::new_with_proxy(proxy.as_ref()).map_err(oauth_error)?;
    let flow = state.oauth_flow();
    let start = flow.start(&oauth).await.map_err(flow_error)?;
    let authorization_url = validated_authorization_url(&start)?;
    if open_browser.unwrap_or(true) && start.status == OAuthFlowStatus::Pending {
        // Browser launch is best effort; the returned URL is the manual fallback.
        let _ = app.opener().open_url(authorization_url, None::<&str>);
    }
    Ok(start)
}

#[tauri::command]
pub async fn resume_codex_oauth(
    app: AppHandle,
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<OAuthFlowStart> {
    let _mutation = state.setup_guard().await;
    let start = state
        .oauth_flow()
        .resume(&login_id)
        .await
        .map_err(flow_error)?;
    let authorization_url = validated_authorization_url(&start)?;
    let _ = app.opener().open_url(authorization_url, None::<&str>);
    Ok(start)
}

#[tauri::command]
pub fn get_codex_oauth_status(
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<OAuthFlowStart> {
    let start = state.oauth_flow().status(&login_id).map_err(flow_error)?;
    validated_authorization_url(&start)?;
    Ok(start)
}

#[tauri::command]
pub async fn submit_codex_oauth_callback(
    login_id: String,
    callback_url: String,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    state
        .oauth_flow()
        .submit_manual_callback(&login_id, &callback_url)
        .await
        .map_err(flow_error)?;
    Ok(())
}

#[tauri::command]
pub async fn cancel_codex_oauth(
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    state
        .oauth_flow()
        .cancel(&login_id)
        .await
        .map_err(flow_error)?;
    Ok(())
}

#[tauri::command]
pub async fn complete_codex_oauth(
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalAccountRecord> {
    let _mutation = state.setup_guard().await;
    complete_oauth(&login_id, &state).await.map_err(Into::into)
}

async fn complete_oauth(login_id: &str, state: &DesktopState) -> LocalResult<LocalAccountRecord> {
    let flow = state.oauth_flow();
    let now_ms = super::current_time_ms();
    let settings = state.store()?.gateway().clone();
    let common_proxy = common_proxy_config(&settings)?;
    ensure_account_proxy(&settings, common_proxy.as_ref())?;
    let (checkpoint, encoded_checkpoint) =
        completion_checkpoint(&flow, login_id, now_ms, common_proxy.as_ref()).await?;
    let (old_accounts, old_keys) = current_accounts(state)?;
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let existing = find_existing_account(
        &old_accounts,
        &credential_store,
        &checkpoint.identity_hash(),
    )?;
    let local_account_id = existing
        .map(|account| account.account.id.clone())
        .unwrap_or_else(|| format!("account_{}", Uuid::new_v4().simple()));
    let previous_credentials = credential_store
        .load(&local_account_id)
        .map_err(credential_error)?;
    let generation = existing
        .map(|account| account.account.token_generation)
        .into_iter()
        .chain(
            previous_credentials
                .as_ref()
                .map(StoredCodexCredentials::generation),
        )
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut credentials = checkpoint
        .to_credentials(&local_account_id, generation)
        .map_err(credential_error)?;
    if let Some(proxy_url) = previous_credentials
        .as_ref()
        .and_then(StoredCodexCredentials::proxy_url)
    {
        credentials = credentials
            .with_proxy_url(Some(proxy_url.to_string()))
            .map_err(credential_error)?;
    }
    let proxy = effective_proxy_config(&settings, &credentials)?;
    if let Some(agent_identity) = previous_credentials
        .as_ref()
        .and_then(StoredCodexCredentials::agent_identity)
    {
        credentials = credentials.with_agent_identity(agent_identity.clone());
    } else {
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Zenith Relay");
        if let Ok(client) = match proxy.as_ref() {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        {
            if let Ok(agent_identity) = AgentIdentityCredential::register_from_oauth(
                &client,
                &checkpoint.access_token,
                checkpoint.account_is_fedramp,
                env!("CARGO_PKG_VERSION"),
            )
            .await
            {
                credentials = credentials.with_agent_identity(agent_identity);
            }
        }
    }
    let previous_models = existing
        .map(|account| account.models.clone())
        .unwrap_or_default();
    let (models, model_issue) = match CodexModelsClient::new_with_proxy(proxy.as_ref()) {
        Ok(client) => match client
            .discover(
                &checkpoint.access_token,
                &checkpoint.provider_account_id,
                zenith_relay_core::providers::chatgpt::CODEX_MODELS_CLIENT_VERSION,
            )
            .await
        {
            Ok(models) if !models.is_empty() => (models, None),
            Ok(_) => (
                previous_models,
                Some(InitialModelIssue {
                    code: "models_empty",
                    retryable: false,
                    auth_error: false,
                    blocked: false,
                }),
            ),
            Err(error) => (previous_models, Some(initial_model_issue(&error))),
        },
        Err(error) => (previous_models, Some(initial_model_issue(&error))),
    };
    let authority_tokens = credentials.to_token_set().map_err(credential_error)?;
    let mut record = new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        models,
        existing.map_or(0, |account| account.priority),
        now_ms,
    )?;
    if let Some(active_until_ms) = checkpoint.subscription_active_until_ms {
        record.account.subscription = zenith_relay_core::quota::Subscription::normalize(
            zenith_relay_core::quota::SubscriptionInput {
                plan_type: record.account.subscription.plan_type.clone(),
                active_until_ms: Some(active_until_ms),
                forbidden: false,
                observed_at_ms: now_ms,
            },
        );
    }
    if let Some(existing) = existing {
        preserve_existing_settings(&mut record, existing);
    }
    let quota_result = match CodexQuotaClient::new_with_proxy_and_timeout(
        proxy.as_ref(),
        std::time::Duration::from_secs(settings.quota_request_timeout_seconds),
    ) {
        Ok(quota) => {
            quota
                .refresh_data_with_subscription(
                    &checkpoint.access_token,
                    &checkpoint.provider_account_id,
                    now_ms,
                    &record.account.subscription,
                    true,
                )
                .await
        }
        Err(error) => Err(error),
    };
    let quota_outcome = match quota_result {
        Ok(data) => match apply_quota_success(&mut record, data) {
            Ok(applied) => AccountQuotaOutcome::Updated {
                transitions: applied.transitions,
            },
            Err(_) => {
                let failure = QuotaRefreshFailure::new("quota_invalid_response", false);
                apply_quota_failure(&mut record, &failure, now_ms);
                AccountQuotaOutcome::Failed {
                    code: failure.code,
                    retryable: failure.retryable,
                }
            }
        },
        Err(failure) => {
            apply_quota_failure(&mut record, &failure, now_ms);
            AccountQuotaOutcome::Failed {
                code: failure.code,
                retryable: failure.retryable,
            }
        }
    };
    if let Some(issue) = model_issue {
        apply_initial_model_issue(&mut record, issue);
    }
    let quota_refresh_at = next_quota_refresh_at(
        &AccountQuotaRefreshResponse {
            account: record.clone(),
            quota: quota_outcome,
        },
        now_ms,
    );

    let runtime_port = state.gateway.address().await.map(|address| address.port());
    credential_store
        .save(&credentials)
        .map_err(credential_error)?;
    let account_write = { state.store()?.upsert_account(record.clone()) };
    if let Err(error) = account_write {
        return Err(rollback_completion(
            state,
            &credential_store,
            &local_account_id,
            previous_credentials.as_ref(),
            &old_accounts,
            &old_keys,
            None,
            error,
        )
        .await);
    }
    if state
        .token_authority()
        .register(
            &local_account_id,
            authority_tokens,
            record.account.auth_state,
        )
        .await
        .is_err()
    {
        return Err(rollback_completion(
            state,
            &credential_store,
            &local_account_id,
            previous_credentials.as_ref(),
            &old_accounts,
            &old_keys,
            None,
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "ChatGPT token authority rejected the account",
            ),
        )
        .await);
    }
    if let Err(error) =
        sync_accounts_or_rollback(state, old_accounts.clone(), old_keys.clone()).await
    {
        return Err(rollback_completion(
            state,
            &credential_store,
            &local_account_id,
            previous_credentials.as_ref(),
            &old_accounts,
            &old_keys,
            None,
            error,
        )
        .await);
    }
    let previous_quota_refresh = match state.quota_refresh_snapshot() {
        Ok(previous) => previous,
        Err(error) => {
            return Err(rollback_completion(
                state,
                &credential_store,
                &local_account_id,
                previous_credentials.as_ref(),
                &old_accounts,
                &old_keys,
                runtime_port,
                error,
            )
            .await)
        }
    };
    let schedule_result = match quota_refresh_at {
        Some(due_at_ms) => state
            .sync_account_quota_refresh(&local_account_id, due_at_ms)
            .map(|_| ()),
        None => state.remove_quota_refresh(&local_account_id).map(|_| ()),
    };
    if let Err(error) = schedule_result {
        return Err(rollback_completion(
            state,
            &credential_store,
            &local_account_id,
            previous_credentials.as_ref(),
            &old_accounts,
            &old_keys,
            runtime_port,
            error,
        )
        .await);
    }
    if let Err(error) = flow.complete(login_id).await.map_err(flow_error) {
        let queue_restored = state.restore_quota_refresh(previous_quota_refresh).is_ok();
        let checkpoint_restored =
            restore_completion_checkpoint(&checkpoint.login_id, &encoded_checkpoint).is_ok();
        let error = if queue_restored && checkpoint_restored {
            error
        } else {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "OAuth completion rollback could not restore pending state",
            )
        };
        return Err(rollback_completion(
            state,
            &credential_store,
            &local_account_id,
            previous_credentials.as_ref(),
            &old_accounts,
            &old_keys,
            runtime_port,
            error,
        )
        .await);
    }
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
async fn rollback_completion(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    local_account_id: &str,
    previous_credentials: Option<&StoredCodexCredentials>,
    old_accounts: &[LocalAccountRecord],
    old_keys: &[LocalGatewayKeyRecord],
    restart_port: Option<u16>,
    cause: LocalPoolError,
) -> LocalPoolError {
    if restart_port.is_some() {
        state.gateway.stop().await;
    }
    let credentials_restored = match previous_credentials {
        Some(previous) => credentials.save(previous),
        None => credentials.delete(local_account_id),
    }
    .is_ok();
    let records_restored = state
        .store()
        .and_then(|mut store| {
            store.replace_accounts_and_keys(old_accounts.to_vec(), old_keys.to_vec())
        })
        .is_ok();
    let previous_account = old_accounts
        .iter()
        .find(|account| account.account.id == local_account_id);
    let authority = state.token_authority();
    let authority_restored = match (previous_credentials, previous_account) {
        (Some(previous), Some(account)) => match previous.to_token_set() {
            Ok(tokens) => authority
                .register(local_account_id, tokens, account.account.auth_state)
                .await
                .is_ok(),
            Err(_) => false,
        },
        _ => {
            authority.remove(local_account_id);
            true
        }
    };
    if !credentials_restored || !records_restored || !authority_restored {
        return super::fail_closed(
            state,
            "OAuth completion rollback could not restore local account state".to_string(),
        )
        .await;
    }
    if let Some(port) = restart_port {
        let Ok(runtime) = super::runtime_from_store(state).await else {
            return super::fail_closed(
                state,
                "OAuth completion rollback could not rebuild the previous runtime".to_string(),
            )
            .await;
        };
        if state.gateway.start(runtime, port).await.is_err() {
            return super::fail_closed(
                state,
                "OAuth completion rollback could not restart the previous runtime".to_string(),
            )
            .await;
        }
    }
    cause
}

fn current_accounts(
    state: &DesktopState,
) -> LocalResult<(Vec<LocalAccountRecord>, Vec<LocalGatewayKeyRecord>)> {
    let store = state.store()?;
    Ok((store.accounts().to_vec(), store.keys().to_vec()))
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OAuthCompletionCheckpoint {
    version: u32,
    login_id: String,
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    issued_at_ms: u64,
    email: Option<String>,
    provider_account_id: String,
    provider_user_id: Option<String>,
    plan_type: Option<String>,
    #[serde(default)]
    subscription_active_until_ms: Option<u64>,
    account_is_fedramp: bool,
}

impl OAuthCompletionCheckpoint {
    fn from_tokens(login_id: &str, tokens: OAuthTokenSet, issued_at_ms: u64) -> LocalResult<Self> {
        let claims = tokens
            .identity_claims()
            .map_err(oauth_error)?
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth response did not contain identity claims",
                )
            })?;
        let provider_account_id = claims
            .account_id()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth response did not contain a ChatGPT account id",
                )
            })?
            .to_string();
        let checkpoint = Self {
            version: COMPLETION_CHECKPOINT_VERSION,
            login_id: login_id.to_string(),
            access_token: tokens.access_token().to_string(),
            refresh_token: tokens.refresh_token().map(str::to_string),
            id_token: tokens.id_token().map(str::to_string),
            expires_at_ms: tokens.expires_at_ms(),
            issued_at_ms,
            email: claims.email().map(str::to_string),
            provider_account_id,
            provider_user_id: claims.user_id().map(str::to_string),
            plan_type: claims.plan_type().map(str::to_string),
            subscription_active_until_ms: claims.subscription_active_until_ms(),
            account_is_fedramp: claims.account_is_fedramp(),
        };
        checkpoint.validate(login_id)?;
        Ok(checkpoint)
    }

    fn validate(&self, expected_login_id: &str) -> LocalResult<()> {
        if self.version != COMPLETION_CHECKPOINT_VERSION
            || self.login_id != expected_login_id
            || self.issued_at_ms == 0
            || self.id_token.is_none()
        {
            return Err(invalid_completion_checkpoint());
        }
        self.to_credentials("oauth_checkpoint", 1)
            .map(|_| ())
            .map_err(|_| invalid_completion_checkpoint())
    }

    fn to_credentials(
        &self,
        local_account_id: &str,
        generation: u64,
    ) -> Result<StoredCodexCredentials, CredentialError> {
        StoredCodexCredentials::new(
            local_account_id,
            self.access_token.clone(),
            self.refresh_token.clone(),
            self.id_token.clone(),
            self.expires_at_ms,
            self.issued_at_ms,
            generation,
            self.email.clone(),
            Some(self.provider_account_id.clone()),
            self.provider_user_id.clone(),
            None,
            self.plan_type.clone(),
            self.account_is_fedramp,
        )
    }

    fn identity_hash(&self) -> String {
        records::identity_hash(
            &self.provider_account_id,
            self.provider_user_id.as_deref(),
            self.email.as_deref(),
        )
    }
}

impl fmt::Debug for OAuthCompletionCheckpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthCompletionCheckpoint")
            .field("version", &self.version)
            .field("login_id", &self.login_id)
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("provider_account_id", &"[redacted]")
            .field(
                "provider_user_id",
                &self.provider_user_id.as_ref().map(|_| "[redacted]"),
            )
            .field("plan_type", &self.plan_type)
            .field(
                "subscription_active_until_ms",
                &self.subscription_active_until_ms,
            )
            .field("account_is_fedramp", &self.account_is_fedramp)
            .finish()
    }
}

async fn completion_checkpoint<E>(
    flow: &OAuthFlowManager<NativeSecretBackend, E>,
    login_id: &str,
    now_ms: u64,
    proxy: Option<&ProxyConfig>,
) -> LocalResult<(OAuthCompletionCheckpoint, String)>
where
    E: OAuthFlowEventSink,
{
    let start = flow.status(login_id).map_err(flow_error)?;
    if start.status != OAuthFlowStatus::CallbackReceived {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OAuth callback has not been received",
        ));
    }
    let secret_ref = callback_secret_ref(&start.login_id);
    let stored = NativeSecretBackend
        .load(&secret_ref)
        .map_err(|_| completion_secret_error())?
        .ok_or_else(completion_secret_error)?;
    if let Some(checkpoint) = decode_completion_checkpoint(&stored, &start.login_id)? {
        return Ok((checkpoint, stored));
    }
    drop(stored);

    let material = flow
        .exchange_material(&start.login_id)
        .map_err(flow_error)?;
    let (pending, callback) = material.into_parts();
    let tokens = CodexOAuthClient::new_with_proxy(proxy)
        .map_err(oauth_error)?
        .exchange_code(&pending, callback, now_ms)
        .await
        .map_err(oauth_error)?;
    let checkpoint = OAuthCompletionCheckpoint::from_tokens(&start.login_id, tokens, now_ms)?;
    let encoded = encode_completion_checkpoint(&checkpoint)?;
    store_completion_checkpoint(&start.login_id, &encoded)?;
    Ok((checkpoint, encoded))
}

fn decode_completion_checkpoint(
    value: &str,
    expected_login_id: &str,
) -> LocalResult<Option<OAuthCompletionCheckpoint>> {
    if !value.trim_start().starts_with('{') {
        return Ok(None);
    }
    if value.len() > MAX_COMPLETION_CHECKPOINT_BYTES {
        return Err(invalid_completion_checkpoint());
    }
    let checkpoint: OAuthCompletionCheckpoint =
        serde_json::from_str(value).map_err(|_| invalid_completion_checkpoint())?;
    checkpoint.validate(expected_login_id)?;
    Ok(Some(checkpoint))
}

fn encode_completion_checkpoint(checkpoint: &OAuthCompletionCheckpoint) -> LocalResult<String> {
    let encoded = serde_json::to_string(checkpoint).map_err(|_| invalid_completion_checkpoint())?;
    if encoded.len() > MAX_COMPLETION_CHECKPOINT_BYTES {
        Err(invalid_completion_checkpoint())
    } else {
        Ok(encoded)
    }
}

fn store_completion_checkpoint(login_id: &str, encoded: &str) -> LocalResult<()> {
    let secret_ref = callback_secret_ref(login_id);
    NativeSecretBackend
        .save(&secret_ref, encoded)
        .map_err(|_| completion_secret_error())?;
    let stored = NativeSecretBackend
        .load(&secret_ref)
        .map_err(|_| completion_secret_error())?;
    if stored.as_deref() != Some(encoded) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "OAuth completion checkpoint could not be verified",
        ));
    }
    Ok(())
}

fn restore_completion_checkpoint(login_id: &str, encoded: &str) -> LocalResult<()> {
    store_completion_checkpoint(login_id, encoded)
}

fn callback_secret_ref(login_id: &str) -> String {
    format!("oauth-callback:{login_id}")
}

fn invalid_completion_checkpoint() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "OAuth completion checkpoint requires recovery",
    )
}

fn completion_secret_error() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::SecretStoreUnavailable,
        "OAuth completion secret storage is unavailable",
    )
}

fn find_existing_account<'a>(
    accounts: &'a [LocalAccountRecord],
    credentials: &CredentialStore<NativeSecretBackend>,
    identity_hash: &str,
) -> LocalResult<Option<&'a LocalAccountRecord>> {
    let direct = accounts
        .iter()
        .filter(|account| {
            account.account.source_id == CODEX_SOURCE_ID
                && account.account.identity.identity_hash == identity_hash
        })
        .collect::<Vec<_>>();
    if direct.len() > 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "multiple local accounts have the same ChatGPT identity",
        ));
    }
    if let Some(account) = direct.into_iter().next() {
        return Ok(Some(account));
    }
    let mut matches = Vec::new();
    for account in accounts {
        if account.account.source_id != CODEX_SOURCE_ID {
            continue;
        }
        let Some(stored) = credentials
            .load(&account.account.id)
            .map_err(credential_error)?
        else {
            continue;
        };
        let Some(provider_account_id) = stored.provider_account_id() else {
            continue;
        };
        if records::identity_hash(
            provider_account_id,
            stored.provider_user_id(),
            stored.email(),
        ) == identity_hash
        {
            matches.push(account);
        }
    }
    if matches.len() > 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "multiple local accounts have the same ChatGPT identity",
        ));
    }
    Ok(matches.pop())
}

fn preserve_existing_settings(next: &mut LocalAccountRecord, current: &LocalAccountRecord) {
    next.account.id = current.account.id.clone();
    next.account.label = current.account.label.clone();
    next.account.tags = current.account.tags.clone();
    next.account.enabled = current.account.enabled;
    next.account.in_pool = current.account.in_pool;
    next.account.draining = current.account.draining;
    next.account.created_at_ms = current.account.created_at_ms;
    next.account.last_used_at_ms = current.account.last_used_at_ms;
    next.account.quota = current.account.quota.clone();
    next.purchase_cost_micro_usd = current.purchase_cost_micro_usd;
    next.remote_location = current.remote_location.clone();
    if next.account.subscription.plan_type.is_none() {
        next.account.subscription.plan_type = current.account.subscription.plan_type.clone();
    }
    if next.account.subscription.active_until_ms.is_none() {
        next.account.subscription.active_until_ms = current.account.subscription.active_until_ms;
    }
    next.allowed_models = current.allowed_models.clone();
    next.excluded_models = current.excluded_models.clone();
    next.priority = current.priority;
    next.weight = current.weight;
}

fn validated_authorization_url(start: &OAuthFlowStart) -> LocalResult<String> {
    let authorization = Url::parse(&start.authorization_url).map_err(|_| unsafe_oauth_url())?;
    let endpoint = Url::parse(AUTHORIZATION_ENDPOINT).map_err(|_| unsafe_oauth_url())?;
    if authorization.scheme() != endpoint.scheme()
        || authorization.host_str() != endpoint.host_str()
        || authorization.port().is_some()
        || authorization.path() != endpoint.path()
        || !authorization.username().is_empty()
        || authorization.password().is_some()
        || authorization.fragment().is_some()
    {
        return Err(unsafe_oauth_url());
    }
    let redirect = Url::parse(&start.redirect_uri).map_err(|_| unsafe_oauth_url())?;
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("localhost")
        || redirect.port().is_none()
        || redirect.path() != CALLBACK_PATH
        || !redirect.username().is_empty()
        || redirect.password().is_some()
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return Err(unsafe_oauth_url());
    }
    let mut seen = BTreeSet::new();
    for (key, value) in authorization.query_pairs() {
        let key = key.into_owned();
        if !seen.insert(key.clone()) {
            return Err(unsafe_oauth_url());
        }
        let valid = match key.as_str() {
            "response_type" => value == "code",
            "client_id" => value == CODEX_OAUTH_CLIENT_ID,
            "redirect_uri" => value == start.redirect_uri,
            "scope" => value == CODEX_OAUTH_SCOPE,
            "code_challenge" | "state" => valid_oauth_nonce(&value),
            "code_challenge_method" => value == "S256",
            "id_token_add_organizations" | "codex_cli_simplified_flow" => value == "true",
            "originator" => value == "codex_cli_rs",
            _ => false,
        };
        if !valid {
            return Err(unsafe_oauth_url());
        }
    }
    if seen.len() != 10 {
        return Err(unsafe_oauth_url());
    }
    Ok(authorization.to_string())
}

fn valid_oauth_nonce(value: &str) -> bool {
    (32..=256).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn unsafe_oauth_url() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "OAuth authorization URL failed validation",
    )
}

fn flow_error(error: OAuthFlowError) -> LocalPoolError {
    let code = match error.code {
        OAuthFlowErrorCode::CallbackAlreadyReceived => ErrorCode::Conflict,
        OAuthFlowErrorCode::Expired | OAuthFlowErrorCode::SecretMissing => ErrorCode::NotFound,
        OAuthFlowErrorCode::InvalidLoginId | OAuthFlowErrorCode::CallbackInvalid => {
            ErrorCode::InvalidState
        }
        OAuthFlowErrorCode::CallbackPortUnavailable | OAuthFlowErrorCode::ListenerUnavailable => {
            ErrorCode::GatewayUnavailable
        }
        OAuthFlowErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        OAuthFlowErrorCode::CleanupIncomplete
        | OAuthFlowErrorCode::RecoveryRequired
        | OAuthFlowErrorCode::SnapshotIo
        | OAuthFlowErrorCode::UnsupportedSnapshotVersion => ErrorCode::RecoveryRequired,
    };
    LocalPoolError::new(code, error.message)
}

fn oauth_error(error: OAuthError) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}

fn credential_error(error: CredentialError) -> LocalPoolError {
    let code = match error.code {
        CredentialErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        CredentialErrorCode::SecretMissing => ErrorCode::NotFound,
        _ => ErrorCode::InvalidState,
    };
    LocalPoolError::new(code, error.to_string())
}

fn initial_model_issue(error: &ModelDiscoveryFailure) -> InitialModelIssue {
    let (code, auth_error, blocked) = match error.code {
        ModelDiscoveryFailureCode::AgentTaskInvalid => ("models_agent_task_invalid", false, false),
        ModelDiscoveryFailureCode::Forbidden => ("models_forbidden", false, true),
        ModelDiscoveryFailureCode::HttpStatus => ("models_http_status", false, false),
        ModelDiscoveryFailureCode::InvalidAccessToken => {
            ("models_invalid_access_token", true, false)
        }
        ModelDiscoveryFailureCode::InvalidAccountId => ("models_invalid_account_id", true, false),
        ModelDiscoveryFailureCode::InvalidClientVersion => {
            ("models_invalid_client_version", false, false)
        }
        ModelDiscoveryFailureCode::InvalidEndpoint => ("models_invalid_endpoint", false, false),
        ModelDiscoveryFailureCode::InvalidResponse => ("models_invalid_response", false, false),
        ModelDiscoveryFailureCode::RateLimited => ("models_rate_limited", false, false),
        ModelDiscoveryFailureCode::ResponseTooLarge => ("models_response_too_large", false, false),
        ModelDiscoveryFailureCode::Transport => ("models_transport", false, false),
        ModelDiscoveryFailureCode::Unauthorized => ("models_unauthorized", true, false),
        ModelDiscoveryFailureCode::Upstream => ("models_upstream", false, false),
    };
    InitialModelIssue {
        code,
        retryable: error.retryable,
        auth_error,
        blocked,
    }
}

fn apply_initial_model_issue(record: &mut LocalAccountRecord, issue: InitialModelIssue) {
    record.account.last_error_code = Some(issue.code.to_string());
    if issue.auth_error {
        record.account.auth_state = AccountAuthState::Error;
        record.account.health = AccountHealthState::Unhealthy;
    } else if issue.blocked {
        record.account.health = AccountHealthState::Blocked;
    } else if !matches!(
        record.account.health,
        AccountHealthState::Blocked | AccountHealthState::Unhealthy
    ) {
        record.account.health = if issue.retryable {
            AccountHealthState::Degraded
        } else {
            AccountHealthState::Unhealthy
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_validation_allows_generated_url_only() {
        let oauth = CodexOAuthClient::new()
            .unwrap()
            .begin(1455, 10_000)
            .unwrap();
        let valid = OAuthFlowStart {
            login_id: Uuid::new_v4().hyphenated().to_string(),
            authorization_url: oauth.authorization_url().to_string(),
            redirect_uri: oauth.pending().redirect_uri().to_string(),
            expires_at_ms: oauth.pending().expires_at_ms(),
            status: OAuthFlowStatus::Pending,
        };
        assert!(validated_authorization_url(&valid).is_ok());

        let mut wrong_host = valid.clone();
        wrong_host.authorization_url = wrong_host
            .authorization_url
            .replace("auth.openai.com", "attacker.invalid");
        assert!(validated_authorization_url(&wrong_host).is_err());

        let mut sensitive = valid.clone();
        let mut url = Url::parse(&sensitive.authorization_url).unwrap();
        url.query_pairs_mut().append_pair("access_token", "secret");
        sensitive.authorization_url = url.to_string();
        let error = validated_authorization_url(&sensitive).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret"));

        let mut override_request = valid.clone();
        let mut url = Url::parse(&override_request.authorization_url).unwrap();
        url.query_pairs_mut()
            .append_pair("request_uri", "https://attacker.invalid/request");
        override_request.authorization_url = url.to_string();
        assert!(validated_authorization_url(&override_request).is_err());

        let mut duplicate_redirect = valid.clone();
        let mut url = Url::parse(&duplicate_redirect.authorization_url).unwrap();
        url.query_pairs_mut()
            .append_pair("redirect_uri", &duplicate_redirect.redirect_uri);
        duplicate_redirect.authorization_url = url.to_string();
        assert!(validated_authorization_url(&duplicate_redirect).is_err());

        let mut wrong_redirect = valid;
        wrong_redirect.redirect_uri = "http://localhost:9999/auth/callback".into();
        assert!(validated_authorization_url(&wrong_redirect).is_err());
    }

    #[test]
    fn exchanged_token_checkpoint_is_recoverable_and_fully_redacted() {
        let login_id = Uuid::new_v4().hyphenated().to_string();
        let checkpoint = OAuthCompletionCheckpoint {
            version: COMPLETION_CHECKPOINT_VERSION,
            login_id: login_id.clone(),
            access_token: "checkpoint-access-secret".into(),
            refresh_token: Some("checkpoint-refresh-secret".into()),
            id_token: Some("checkpoint-id-secret".into()),
            expires_at_ms: Some(60_000),
            issued_at_ms: 1,
            email: Some("private@example.test".into()),
            provider_account_id: "provider-private-id".into(),
            provider_user_id: Some("provider-user-private-id".into()),
            plan_type: Some("plus".into()),
            subscription_active_until_ms: Some(1_788_998_400_000),
            account_is_fedramp: false,
        };
        let encoded = encode_completion_checkpoint(&checkpoint).unwrap();
        let recovered = decode_completion_checkpoint(&encoded, &login_id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.access_token, "checkpoint-access-secret");
        assert!(decode_completion_checkpoint(
            "http://localhost:1455/auth/callback?code=callback-secret",
            &login_id
        )
        .unwrap()
        .is_none());
        let rendered = format!("{recovered:?}");
        for secret in [
            "checkpoint-access-secret",
            "checkpoint-refresh-secret",
            "checkpoint-id-secret",
            "private@example.test",
            "provider-private-id",
            "provider-user-private-id",
        ] {
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn failed_initial_probe_keeps_account_with_typed_error() {
        let mut record = account("account_saved", "provider-account", "refresh-token");
        record.models.clear();
        apply_initial_model_issue(
            &mut record,
            initial_model_issue(&ModelDiscoveryFailure {
                code: ModelDiscoveryFailureCode::Unauthorized,
                retryable: false,
                http_status: Some(401),
            }),
        );

        assert_eq!(record.account.id, "account_saved");
        assert!(record.models.is_empty());
        assert_eq!(record.account.auth_state, AccountAuthState::Error);
        assert_eq!(record.account.health, AccountHealthState::Unhealthy);
        assert_eq!(
            record.account.last_error_code.as_deref(),
            Some("models_unauthorized")
        );
    }

    #[test]
    fn duplicate_identity_preserves_local_id_and_user_settings() {
        let mut current = account("account_existing", "provider-account", "old-refresh");
        current.account.label = "My Codex".into();
        current.account.tags = BTreeSet::from(["work".into()]);
        current.account.enabled = false;
        current.account.in_pool = true;
        current.account.draining = true;
        current.account.created_at_ms = 7;
        current.account.last_used_at_ms = Some(8);
        current.allowed_models = vec!["allowed".into()];
        current.excluded_models = vec!["excluded".into()];
        current.priority = -10;
        current.weight = 4;
        current.purchase_cost_micro_usd = Some(42_000_000);
        current.cooldowns.insert("gpt-test".into(), 900);
        current.consecutive_failures = 3;

        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let identity_hash =
            records::identity_hash("provider-account", None, Some("private@example.test"));
        let existing =
            find_existing_account(std::slice::from_ref(&current), &credentials, &identity_hash)
                .unwrap()
                .unwrap();
        let mut next = account("account_existing", "provider-account", "new-refresh");
        next.models = vec!["new-model".into()];
        preserve_existing_settings(&mut next, existing);

        assert_eq!(next.account.id, "account_existing");
        assert_eq!(next.account.label, "My Codex");
        assert_eq!(
            next.account.identity.identity_hash,
            current.account.identity.identity_hash
        );
        assert_ne!(
            next.account.identity.stable_index,
            current.account.identity.stable_index
        );
        assert_eq!(next.account.tags, current.account.tags);
        assert!(!next.account.enabled);
        assert!(next.account.in_pool);
        assert!(next.account.draining);
        assert_eq!(next.account.created_at_ms, 7);
        assert_eq!(next.account.last_used_at_ms, Some(8));
        assert_eq!(next.allowed_models, vec!["allowed"]);
        assert_eq!(next.excluded_models, vec!["excluded"]);
        assert_eq!(next.priority, -10);
        assert_eq!(next.weight, 4);
        assert_eq!(next.purchase_cost_micro_usd, Some(42_000_000));
        assert!(next.cooldowns.is_empty());
        assert_eq!(next.consecutive_failures, 0);
        assert_eq!(next.models, vec!["new-model"]);
    }

    #[test]
    fn duplicate_identity_conflict_is_redacted() {
        let provider_account_id = "provider-private-id";
        let accounts = vec![
            account("account_one", provider_account_id, "refresh-one"),
            account("account_two", provider_account_id, "refresh-two"),
        ];
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let identity_hash =
            records::identity_hash(provider_account_id, None, Some("private@example.test"));
        let error = find_existing_account(&accounts, &credentials, &identity_hash).unwrap_err();
        assert!(matches!(error.code, ErrorCode::RecoveryRequired));
        assert!(!format!("{error:?} {error}").contains(provider_account_id));
    }

    fn account(id: &str, provider_account_id: &str, refresh_token: &str) -> LocalAccountRecord {
        let credentials = StoredCodexCredentials::new(
            id,
            "access-secret".into(),
            Some(refresh_token.into()),
            Some("id-secret".into()),
            Some(60_000),
            1,
            1,
            Some("private@example.test".into()),
            Some(provider_account_id.into()),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .unwrap();
        new_account_record(
            &credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            1,
        )
        .unwrap()
    }
}

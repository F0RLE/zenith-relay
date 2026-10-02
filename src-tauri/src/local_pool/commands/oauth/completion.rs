use crate::local_pool::{
    accounts::{
        authority::{ProcessAccountLocks, ProcessLockConfig},
        credentials::{
            credential_local_error as credential_error, CredentialStore, StoredCodexCredentials,
        },
        proxy::{common_proxy_config, effective_proxy_config, ensure_account_proxy},
        quota_refresh::{
            register_active_authority, AccountQuotaOutcome, AccountQuotaRefreshResponse,
        },
        quota_service::{apply_quota_failure, apply_quota_success},
        records::new_account_record,
        NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::LocalAccountRecord,
    state::DesktopState,
};
use reqwest::redirect::Policy;

use uuid::Uuid;
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    accounts::AccountAuthMode,
    providers::chatgpt::{AgentIdentityCredential, CodexModelsClient, CodexQuotaClient},
    quota::QuotaRefreshFailure,
    ProxyConfig,
};

use super::account::{
    apply_initial_model_issue, find_existing_account, initial_model_issue,
    preserve_existing_settings, InitialModelIssue,
};
use super::checkpoint::{
    completion_checkpoint, restore_completion_checkpoint, OAuthCompletionCheckpoint,
};
use super::flow::flow_error;

struct PreparedOAuthCompletion {
    now_ms: u64,
    sign_in_proxy_url: Option<String>,
    checkpoint: OAuthCompletionCheckpoint,
    encoded_checkpoint: String,
    had_existing: bool,
    credentials: StoredCodexCredentials,
    record: LocalAccountRecord,
    local_account_id: String,
    account_hash: String,
    quota_reset_delay: Option<u64>,
}

pub(super) async fn complete_oauth(
    login_id: &str,
    state: &DesktopState,
) -> LocalResult<LocalAccountRecord> {
    let prepared = prepare_oauth_completion(login_id, state).await?;
    commit_oauth_completion(login_id, state, prepared).await
}

async fn prepare_oauth_completion(
    login_id: &str,
    state: &DesktopState,
) -> LocalResult<PreparedOAuthCompletion> {
    let login_hash = crate::diagnostics::hash_identifier(login_id);
    crate::diagnostics::breadcrumb(
        "oauth",
        "completion_checkpoint",
        &[("login", login_hash.clone())],
    );
    let flow = state.oauth_flow();
    let now_ms = super::super::current_time_ms();
    let settings = state.store()?.gateway().clone();
    let sign_in_proxy_url = match flow.sign_in_proxy_id(login_id).map_err(flow_error)? {
        Some(proxy_id) => Some(super::http_sign_in_proxy_url(&proxy_id)?),
        None => None,
    };
    let exchange_proxy = if let Some(url) = sign_in_proxy_url.as_deref() {
        let proxy = super::parsed_proxy(Some(url))?;
        ensure_account_proxy(&settings, proxy.as_ref())?;
        proxy
    } else {
        let common_proxy = common_proxy_config(&settings)?;
        ensure_account_proxy(&settings, common_proxy.as_ref())?;
        common_proxy
    };
    let (checkpoint, encoded_checkpoint, target_account_id) =
        completion_checkpoint(&flow, login_id, now_ms, exchange_proxy.as_ref()).await?;
    let old_accounts = current_accounts(state)?;
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let existing = if let Some(target_account_id) = target_account_id.as_deref() {
        let target = old_accounts
            .iter()
            .find(|account| account.account.id == target_account_id)
            .ok_or_else(|| {
                LocalPoolError::new(ErrorCode::NotFound, "OAuth target account was not found")
            })?;
        if target.remote_location.is_some() {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "OAuth target account is managed by a remote server",
            ));
        }
        let stored_provider_id = credential_store
            .load(target_account_id)
            .map_err(credential_error)?
            .and_then(|credentials| credentials.provider_account_id().map(str::to_string));
        if stored_provider_id
            .as_deref()
            .is_some_and(|provider_id| provider_id != checkpoint.provider_account_id)
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "OAuth account does not match the selected local account",
            ));
        }
        Some(target)
    } else {
        find_existing_account(
            &old_accounts,
            &credential_store,
            &checkpoint.identity_hash(),
        )?
    };
    let local_account_id = existing
        .map(|account| account.account.id.clone())
        .unwrap_or_else(|| format!("account_{}", Uuid::new_v4().simple()));
    let account_hash = crate::diagnostics::hash_identifier(&local_account_id);
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
    if let Some(previous) = previous_credentials.as_ref() {
        credentials = inherit_session_material(credentials, previous)?;
    }
    credentials = with_sign_in_proxy(credentials, sign_in_proxy_url.as_deref())?;
    let proxy = effective_proxy_config(&settings, &credentials)?;
    credentials = register_agent_identity_if_missing(
        credentials,
        proxy.as_ref(),
        &checkpoint.access_token,
        checkpoint.account_is_fedramp,
    )
    .await;
    let previous_models = existing
        .map(|account| account.effective_models().to_vec())
        .unwrap_or_default();
    let (models, model_issue) = discover_sign_in_models(
        proxy.as_ref(),
        &credentials,
        &checkpoint.provider_account_id,
        now_ms,
        previous_models,
    )
    .await?;
    crate::diagnostics::breadcrumb(
        "oauth",
        "initial_probes_complete",
        &[
            ("login", login_hash),
            ("models_found", models.len().to_string()),
            ("model_issue", model_issue.is_some().to_string()),
        ],
    );
    let mut record = new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        models,
        existing.map_or(0, |account| account.priority),
        now_ms,
    )?;
    let model_discovery_succeeded = model_issue.is_none();
    if model_discovery_succeeded {
        // Preserve an explicit empty discovery result. It is different from
        // a failed probe and must suppress an older configured catalog.
        record.discovered_models = Some(record.models.clone());
    }
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
    let quota_outcome = refresh_sign_in_quota(
        &mut record,
        proxy.as_ref(),
        &checkpoint.access_token,
        &checkpoint.provider_account_id,
        now_ms,
        std::time::Duration::from_secs(settings.quota_request_timeout_seconds),
    )
    .await;
    if let Some(issue) = model_issue {
        apply_initial_model_issue(&mut record, issue);
    }
    let quota_reset_delay = crate::local_pool::refresh::reset_due_delay(
        &AccountQuotaRefreshResponse {
            account: record.clone(),
            quota: quota_outcome,
            exhaustion_transitions: Vec::new(),
        },
        now_ms,
    );
    let had_existing = existing.is_some();
    Ok(PreparedOAuthCompletion {
        now_ms,
        sign_in_proxy_url,
        checkpoint,
        encoded_checkpoint,
        had_existing,
        credentials,
        record,
        local_account_id,
        account_hash,
        quota_reset_delay,
    })
}

async fn register_agent_identity_if_missing(
    credentials: StoredCodexCredentials,
    proxy: Option<&ProxyConfig>,
    access_token: &str,
    account_is_fedramp: bool,
) -> StoredCodexCredentials {
    if credentials.agent_identity().is_some() {
        return credentials;
    }
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Zenith Relay");
    let Ok(client) = match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build() else {
        return credentials;
    };
    let Ok(agent_identity) = AgentIdentityCredential::register_from_oauth(
        &client,
        access_token,
        account_is_fedramp,
        env!("CARGO_PKG_VERSION"),
    )
    .await
    else {
        return credentials;
    };
    credentials.with_agent_identity(agent_identity)
}

async fn discover_sign_in_models(
    proxy: Option<&ProxyConfig>,
    credentials: &StoredCodexCredentials,
    provider_account_id: &str,
    now_ms: u64,
    previous_models: Vec<String>,
) -> LocalResult<(Vec<String>, Option<InitialModelIssue>)> {
    let client_version = zenith_relay_core::providers::chatgpt::configured_codex_client_version();
    match CodexModelsClient::new_with_proxy(proxy) {
        Ok(client) => match client
            .discover_authorized(
                credentials
                    .authorization(now_ms)
                    .map_err(credential_error)?,
                provider_account_id,
                &client_version,
            )
            .await
        {
            Ok(models) => Ok((models, None)),
            Err(error) => Ok((previous_models, Some(initial_model_issue(&error)))),
        },
        Err(error) => Ok((previous_models, Some(initial_model_issue(&error)))),
    }
}

async fn refresh_sign_in_quota(
    record: &mut LocalAccountRecord,
    proxy: Option<&ProxyConfig>,
    access_token: &str,
    provider_account_id: &str,
    now_ms: u64,
    timeout: std::time::Duration,
) -> AccountQuotaOutcome {
    let quota_result = match CodexQuotaClient::new_with_proxy_and_timeout(proxy, timeout) {
        Ok(quota) => {
            quota
                .refresh_data_with_subscription(
                    access_token,
                    provider_account_id,
                    now_ms,
                    &record.account.subscription,
                    true,
                )
                .await
        }
        Err(error) => Err(error),
    };
    match quota_result {
        Ok(data) => match apply_quota_success(record, data) {
            Ok(applied) => AccountQuotaOutcome::Updated {
                transitions: applied.transitions,
                exhaustion_transitions: applied.exhaustion_transitions,
            },
            Err(_) => {
                let failure = QuotaRefreshFailure::new(error_codes::QUOTA_INVALID_RESPONSE, false);
                apply_quota_failure(record, &failure, now_ms);
                AccountQuotaOutcome::Failed {
                    code: failure.code,
                    retryable: failure.retryable,
                }
            }
        },
        Err(failure) => {
            apply_quota_failure(record, &failure, now_ms);
            AccountQuotaOutcome::Failed {
                code: failure.code,
                retryable: failure.retryable,
            }
        }
    }
}

async fn commit_oauth_completion(
    login_id: &str,
    state: &DesktopState,
    prepared: PreparedOAuthCompletion,
) -> LocalResult<LocalAccountRecord> {
    let PreparedOAuthCompletion {
        now_ms,
        sign_in_proxy_url,
        checkpoint,
        encoded_checkpoint,
        had_existing,
        credentials,
        mut record,
        local_account_id,
        account_hash,
        quota_reset_delay,
    } = prepared;
    let flow = state.oauth_flow();
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    // OAuth exchange and initial probes can take long enough for a background
    // refresh or a desktop-profile synchronization to rotate the same
    // account. Serialize only the final durable credential write, then derive
    // its generation from the fresh snapshot while holding the shared lock.
    // Do not await TokenAuthority while holding this lock: its refresh adapter
    // deliberately acquires the locks in the opposite order.
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth credential lock is unavailable",
                )
            })?;
    let commit_guard = locks.acquire(&local_account_id).await.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::Conflict,
            "account credentials are being refreshed",
        )
    })?;
    let commit_previous_credentials = credential_store
        .load(&local_account_id)
        .map_err(credential_error)?;
    if commit_previous_credentials
        .as_ref()
        .and_then(StoredCodexCredentials::provider_account_id)
        .is_some_and(|provider_id| provider_id != checkpoint.provider_account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "OAuth account changed while completing sign-in",
        ));
    }
    let commit_previous_account = state.store()?.account(&local_account_id).cloned();
    if had_existing && commit_previous_account.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "OAuth target account changed while completing sign-in",
        ));
    }
    let generation = next_completion_generation(
        commit_previous_account.as_ref(),
        commit_previous_credentials.as_ref(),
    );
    let template = commit_previous_credentials.as_ref().unwrap_or(&credentials);
    let committed_credentials = inherit_session_material(
        checkpoint
            .to_credentials(&local_account_id, generation)
            .map_err(credential_error)?,
        template,
    )?;
    let committed_credentials =
        with_sign_in_proxy(committed_credentials, sign_in_proxy_url.as_deref())?;
    if let Some(current) = commit_previous_account.as_ref() {
        preserve_existing_settings(&mut record, current);
    }
    record.account.token_generation = committed_credentials.generation();
    record.account.token_updated_at_ms = Some(committed_credentials.issued_at_ms());
    let authority_tokens = committed_credentials
        .to_token_set()
        .map_err(credential_error)?;
    // A replaced login may change the provider principal while an old
    // request is waiting to send. Fence that account before committing the
    // credential and keep it closed through runtime replacement or restore.
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = super::super::fence_runtime_candidates(
        runtime.as_deref(),
        std::slice::from_ref(&local_account_id),
        &[],
    );
    state
        .store()?
        .invalidate_account_refresh(&[&local_account_id])?;
    credential_store
        .save(&committed_credentials)
        .map_err(credential_error)?;
    crate::diagnostics::breadcrumb(
        "oauth",
        "credentials_committed",
        &[("account", account_hash.clone())],
    );
    let account_write = state.store()?.upsert_account(record.clone());
    if let Err(error) = account_write {
        let rollback = rollback_completion_before_authority(
            state,
            &credential_store,
            &local_account_id,
            commit_previous_credentials.as_ref(),
            commit_previous_account.as_ref(),
            &committed_credentials,
            &record,
        );
        drop(commit_guard);
        return Err(match rollback {
            Ok(true) => error,
            Ok(false) => {
                super::super::fail_closed(
                    state,
                    "OAuth completion was superseded before runtime synchronization".into(),
                )
                .await
            }
            Err(_) => {
                super::super::fail_closed(
                    state,
                    "OAuth completion could not restore the previous account state".into(),
                )
                .await
            }
        });
    }
    drop(commit_guard);

    let registered = register_active_authority(
        state,
        &local_account_id,
        authority_tokens.clone(),
        record.account.auth_state,
        "failed to register OAuth account credentials",
        "OAuth account token state disappeared",
        "OAuth account authentication state disappeared",
    )
    .await?;
    let authoritative_tokens = registered.tokens;
    let authoritative_auth_state = registered.auth_state;
    let authority_state_changed = authoritative_tokens != authority_tokens
        || authoritative_auth_state != record.account.auth_state;
    if authority_state_changed
        && reconcile_completion_authority(
            state,
            &local_account_id,
            &record,
            &authoritative_tokens,
            authoritative_auth_state,
        )
        .is_err()
    {
        return Err(super::super::fail_closed(
            state,
            "newer OAuth account state could not be persisted".to_string(),
        )
        .await);
    }
    // The OAuth credentials are committed at this point. Runtime failures must
    // not restore a stale account snapshot over a later refresh/login; retry
    // the current state without a data rollback instead.
    super::super::restart_or_rollback(state, || Ok(())).await?;
    drop(_dispatch_fences);
    state.sync_account_quota_refresh(
        &local_account_id,
        now_ms.saturating_add(quota_reset_delay.unwrap_or(15 * 60_000)),
    )?;
    if let Err(error) = flow.complete(login_id).await.map_err(flow_error) {
        // Keep registrations derived from committed credentials/current state.
        state.store()?.notify_refresh_changed();
        let checkpoint_restored =
            restore_completion_checkpoint(&checkpoint.login_id, &encoded_checkpoint).is_ok();
        let error = if checkpoint_restored {
            error
        } else {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "OAuth completion rollback could not restore pending state",
            )
        };
        return Err(error);
    }
    crate::diagnostics::record_operation("oauth", "completed", &[("account", account_hash)]);
    Ok(record)
}

fn inherit_session_material(
    mut credentials: StoredCodexCredentials,
    template: &StoredCodexCredentials,
) -> LocalResult<StoredCodexCredentials> {
    if let Some(proxy_url) = template.proxy_url() {
        credentials = credentials
            .with_proxy_url(Some(proxy_url.to_string()))
            .map_err(credential_error)?;
    }
    if let Some(agent_identity) = template.agent_identity() {
        credentials = credentials.with_agent_identity(agent_identity.clone());
    }
    Ok(credentials)
}

fn with_sign_in_proxy(
    credentials: StoredCodexCredentials,
    proxy_url: Option<&str>,
) -> LocalResult<StoredCodexCredentials> {
    match proxy_url {
        Some(url) => credentials
            .with_proxy_url(Some(url.to_string()))
            .map_err(credential_error),
        None => Ok(credentials),
    }
}

#[allow(clippy::too_many_arguments)]
mod rollback;

#[cfg(test)]
pub(super) use rollback::{
    completion_rollback_owns_state, restore_attempted_completion_credentials_if_current,
};
pub(super) use rollback::{
    current_accounts, next_completion_generation, reconcile_completion_authority,
    rollback_completion_before_authority,
};

use super::*;

mod failure;

pub(in crate::local_pool::accounts) use failure::{
    classify_manual_refresh_failure, is_credential_refresh_error_code,
    persist_manual_refresh_failure, persisted_token_generation_is_newer, token_set_is_newer,
};

pub(in crate::local_pool::accounts) async fn force_refresh_account_credentials(
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
        let (mut store, mut updated_account) = super::load_stored_account(state, account_id)?;
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
    let registered = super::register_active_authority(
        state,
        account_id,
        tokens.clone(),
        AccountAuthState::Active,
        "failed to register refreshed credentials",
        "refreshed account token state disappeared",
        "refreshed account authentication state disappeared",
    )
    .await?;
    project_registered_account(
        state,
        account_id,
        &tokens,
        &registered.tokens,
        registered.auth_state,
        AccountProjectionErrors {
            persist: "newer refreshed account state could not be persisted",
            missing: "refreshed account state disappeared",
            policy: "refreshed account state could not update the running account policy",
        },
    )
    .await?;
    // A successful token refresh does not prove that the official desktop
    // client left its login page. Keep the watchdog observation until the
    // client itself reports an available state; otherwise this action could
    // hide a real login redirect that happened while the refresh was running.
    Ok(CredentialRefreshResult {
        account_id: account_id.to_string(),
        status: CredentialRefreshStatus::Refreshed,
        code: "credentials_refreshed".to_string(),
        expires_at_ms: registered.tokens.expires_at_ms(),
        generation: Some(registered.tokens.generation()),
    })
}

pub(crate) fn sync_account_profile_bindings(
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
pub(in crate::local_pool::accounts) async fn rollback_force_refreshed_before_authority(
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

pub(in crate::local_pool::accounts) fn restore_force_refreshed_account_record(
    state: &DesktopState,
    account_id: &str,
    attempted_tokens: &TokenSet,
    old_accounts: &[LocalAccountRecord],
) -> LocalResult<bool> {
    let previous = old_accounts
        .iter()
        .find(|account| account.account.id == account_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let (mut store, mut current) = super::load_stored_account(state, account_id)?;
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

pub(in crate::local_pool::accounts) fn reconcile_force_refreshed_account_record(
    state: &DesktopState,
    account_id: &str,
    expected_tokens: &TokenSet,
    authoritative_tokens: &TokenSet,
    authoritative_auth_state: AccountAuthState,
) -> LocalResult<()> {
    let (mut store, mut account) = super::load_stored_account(state, account_id)?;
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

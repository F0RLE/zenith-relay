use super::*;

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
        .is_some_and(|refreshed_account_id| refreshed_account_id != provider_account_id)
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
        let (mut store, mut account) = super::load_stored_account(state, account_id)?;
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
    let registered = super::register_active_authority(
        state,
        account_id,
        tokens.clone(),
        AccountAuthState::Active,
        "failed to register managed ChatGPT tokens",
        "managed ChatGPT token state disappeared",
        "managed ChatGPT authentication state disappeared",
    )
    .await?;
    project_registered_account(
        state,
        account_id,
        &tokens,
        &registered.tokens,
        registered.auth_state,
        AccountProjectionErrors {
            persist: "newer managed ChatGPT state could not be persisted",
            missing: "managed ChatGPT account state disappeared",
            policy: "managed ChatGPT state could not update the running account policy",
        },
    )
    .await?;
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
        &registered.tokens,
        &provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        &registered.tokens,
        &provider_account_id,
    )?;
    Ok(true)
}

use super::super::*;

pub(in crate::local_pool::accounts) fn token_set_is_newer(
    candidate: &TokenSet,
    current: &TokenSet,
) -> bool {
    candidate.generation() > current.generation()
        || (candidate.generation() == current.generation()
            && candidate.issued_at_ms() > current.issued_at_ms())
}

pub(in crate::local_pool::accounts) fn is_credential_refresh_error_code(code: &str) -> bool {
    matches!(
        code,
        error_codes::INVALID_GRANT
            | error_codes::REFRESH_TOKEN_MISSING
            | error_codes::REFRESH_TOKEN_EXPIRED
            | error_codes::REFRESH_TOKEN_INVALIDATED
            | error_codes::TOKEN_INVALIDATED
    ) || code.starts_with("auth_")
}

pub(in crate::local_pool::accounts) fn classify_manual_refresh_failure(
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

pub(in crate::local_pool::accounts) async fn persist_manual_refresh_failure(
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
    let (mut store, mut account) = super::super::load_stored_account(state, account_id)?;
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
    let (mut store, mut account) = super::super::load_stored_account(state, account_id)?;
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

pub(in crate::local_pool::accounts) fn persisted_token_generation_is_newer(
    persisted_generation: u64,
    persisted_updated_at_ms: Option<u64>,
    expected_tokens: &TokenSet,
) -> bool {
    persisted_generation > expected_tokens.generation()
        || (persisted_generation == expected_tokens.generation()
            && persisted_updated_at_ms
                .is_some_and(|updated_at_ms| updated_at_ms > expected_tokens.issued_at_ms()))
}

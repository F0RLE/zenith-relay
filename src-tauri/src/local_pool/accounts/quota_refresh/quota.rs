use super::*;

pub(in crate::local_pool::accounts) async fn refresh_account_quotas(
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

pub(in crate::local_pool::accounts) async fn refresh_manual_account_quota(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<AccountQuotaRefreshResponse> {
    refresh_account_quota_once(state, account_id).await
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

pub(in crate::local_pool::accounts) async fn prepare_account_credentials_with_remote_policy(
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
    super::profile::sync_managed_account_profile(state, account_id).await?;
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

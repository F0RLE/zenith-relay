use super::super::*;

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

pub(in crate::local_pool::accounts) async fn ensure_local_agent_identity_task(
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

pub(in crate::local_pool::accounts) fn quota_refresh_has_invalid_agent_task(
    result: &std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
) -> bool {
    matches!(
        result,
        Ok(QuotaRefreshOutcome::Failed { failure, .. })
            if is_agent_identity_task_invalid_failure(failure)
    )
}

pub(in crate::local_pool::accounts) fn model_discovery_has_invalid_agent_task(
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

pub(in crate::local_pool::accounts) fn quota_refresh_was_unauthorized(
    result: &std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
) -> bool {
    matches!(
        result,
        Ok(QuotaRefreshOutcome::Failed { failure, .. })
            if failure.http_status() == Some(401)
    )
}

pub(in crate::local_pool::accounts) fn model_discovery_was_unauthorized(
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

pub(in crate::local_pool::accounts) async fn recover_account_authorization(
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

pub(in crate::local_pool::accounts) fn account_requires_reauthentication(
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

pub(in crate::local_pool::accounts) fn account_auth_is_access_only(
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

pub(in crate::local_pool::accounts) async fn mark_access_only_reauthentication(
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

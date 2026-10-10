use super::super::*;
use super::{
    account_auth_is_access_only, account_requires_reauthentication,
    ensure_local_agent_identity_task, mark_access_only_reauthentication,
    model_discovery_has_invalid_agent_task, model_discovery_was_unauthorized,
    prepare_account_request_authorization, recover_account_authorization,
};

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
    let models_result = read_account_models(state, scope).await;
    if let Err(error) = &models_result {
        record_read_error(state, scope, RefreshReadKind::Models, error).await;
    }
    models_result
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
    sync_refreshed_account_or_rollback(
        state,
        applied.previous_account,
        applied.account,
        applied.refresh_result,
    )
    .await?;
    Ok(succeeded)
}

fn respect_models_retry_after(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    model_refresh_result: &std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) {
    if let Some(delay) = model_refresh_result
        .as_ref()
        .err()
        .and_then(|failure| failure.retry_after_ms)
    {
        state
            .refresh
            .respect_retry_after(&scope.fence.identity(), RefreshKind::Models, delay);
    }
}

pub(in crate::local_pool::accounts) async fn discover_account_models(
    prepared: &PreparedAccountAuthorization,
    http_scope: &ManagementHttpScope,
) -> std::result::Result<Vec<String>, ModelDiscoveryFailure> {
    let client = AccountModelsClient::new_with_proxy(prepared.proxy.as_ref())?
        .with_oauth_client_kind(prepared.oauth_client_kind)
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

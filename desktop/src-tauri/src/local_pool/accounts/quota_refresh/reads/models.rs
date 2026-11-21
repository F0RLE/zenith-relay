use super::super::*;
use super::DesktopMonitor;
use zenith_relay_core::providers::chatgpt::connector::monitoring;

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
    let models_result = monitoring::refresh_models(&DesktopMonitor { state, scope }).await;
    if let Err(error) = &models_result {
        record_read_error(state, scope, RefreshReadKind::Models, error).await;
    }
    models_result
}

pub(in crate::local_pool::accounts) async fn discover_account_models(
    state: &DesktopState,
    account_id: &str,
    prepared: &PreparedAccountAuthorization,
    http_scope: &ManagementHttpScope,
) -> std::result::Result<Vec<String>, ModelDiscoveryFailure> {
    let unavailable = || ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::Transport,
        retryable: true,
        http_status: None,
        retry_after_ms: None,
    };
    state
        .store()
        .map_err(|_| unavailable())?
        .account(account_id)
        .ok_or_else(unavailable)?;
    let client = AccountModelsClient::new_with_proxy(prepared.proxy.as_ref())?
        .with_oauth_client_kind(prepared.oauth_client_kind)
        .with_http_scope(http_scope.clone());
    let client_version = zenith_relay_core::providers::chatgpt::configured_codex_client_version();
    let result = client
        .discover_authorized_with_diagnostics(
            prepared.authorization.clone(),
            &prepared.provider_account_id,
            &client_version,
            crate::diagnostics::is_debug_enabled(),
        )
        .await;
    match &result {
        Ok(discovery) => {
            if let Some(catalog) = &discovery.basis_points_diagnostics {
                crate::local_pool::accounts::model_probe::record_catalog(
                    account_id,
                    prepared.proxy.is_some(),
                    catalog,
                );
            }
        }
        Err(error) => crate::local_pool::accounts::model_probe::record_failure(
            account_id,
            prepared.oauth_client_kind,
            prepared.proxy.is_some(),
            error,
        ),
    }
    result.map(|discovery| discovery.models)
}

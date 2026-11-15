use super::super::{
    NativeSecretBackend,
    credentials::CredentialStore,
    import_orchestrator::credential_local_error,
    quota_refresh::{
        PreparedAccountAuthorization, ensure_local_agent_identity_task,
        prepare_account_request_authorization, recover_account_authorization,
    },
};
use super::ResetHttpResponse;
use crate::local_pool::{
    error::{ErrorCode, ErrorDiagnostics, LocalPoolError, Result as LocalResult},
    runtime::current_time_ms,
    state::DesktopState,
};
#[cfg(test)]
use reqwest::StatusCode;
use zenith_relay_core::providers::chatgpt::connector::reset_credits as shared;

fn authorization(prepared: &PreparedAccountAuthorization) -> shared::ResetAuthorization<'_> {
    shared::ResetAuthorization {
        client_kind: prepared.oauth_client_kind,
        authorization: &prepared.authorization,
        provider_account_id: &prepared.provider_account_id,
        proxy: prepared.proxy.as_ref(),
    }
}

pub(super) fn local_error(error: shared::ResetCreditsError) -> LocalPoolError {
    let code = match error.kind {
        shared::ResetErrorKind::Conflict => ErrorCode::Conflict,
        shared::ResetErrorKind::InvalidState => ErrorCode::InvalidState,
        shared::ResetErrorKind::Unavailable => ErrorCode::GatewayUnavailable,
    };
    let result = LocalPoolError::new(code, error.message);
    if error.status.is_some() {
        result.with_diagnostic(ErrorDiagnostics {
            status: error.status,
            retryable: error.retryable,
            ..ErrorDiagnostics::default()
        })
    } else {
        result
    }
}

pub(super) async fn get_reset_credits(
    prepared: &PreparedAccountAuthorization,
) -> LocalResult<ResetHttpResponse> {
    shared::get_reset_credits(&authorization(prepared))
        .await
        .map_err(local_error)
}

pub(super) async fn retry_authorization(
    state: &DesktopState,
    account_id: &str,
    prepared: &PreparedAccountAuthorization,
) -> LocalResult<PreparedAccountAuthorization> {
    if prepared.tokens.is_some() {
        return PreparedAccountAuthorization::from_tokens(
            recover_account_authorization(state, account_id, prepared, current_time_ms()).await?,
        );
    }
    let stored = CredentialStore::from_backend(NativeSecretBackend)
        .require(account_id)
        .map_err(credential_local_error)?;
    let Some(task_id) = prepared.agent_task_id.as_deref() else {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account authorization was rejected and cannot be renewed",
        ));
    };
    ensure_local_agent_identity_task(state, account_id, stored, Some(task_id)).await?;
    prepare_account_request_authorization(state, account_id).await
}

pub(super) async fn post_reset_credit(
    prepared: &PreparedAccountAuthorization,
    request_id: &str,
) -> LocalResult<ResetHttpResponse> {
    shared::post_reset_credit(&authorization(prepared), request_id)
        .await
        .map_err(local_error)
}

#[cfg(test)]
pub(super) fn reset_http_error(status: StatusCode) -> LocalPoolError {
    local_error(shared::reset_http_error(status))
}

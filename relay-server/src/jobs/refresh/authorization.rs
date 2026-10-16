use super::*;

pub(super) async fn prepare_authorization(
    state: &Arc<AppState>,
    fence: &AccountRefreshFence,
) -> Result<PreparedAuthorization, AuthorizationFailure> {
    let (account, stored_fence) = state
        .store
        .account_refresh_scope(&fence.account_id)
        .map_err(|_| AuthorizationFailure::Stale)?;
    if &stored_fence != fence {
        return Err(AuthorizationFailure::Stale);
    }
    let secret = state
        .vault
        .load(&account.secret_ref)
        .map_err(|_| AuthorizationFailure::SecretLoad)?
        .ok_or(AuthorizationFailure::SecretMissing)?;
    let credential: AccountCredential =
        serde_json::from_str(&secret).map_err(|_| AuthorizationFailure::SecretInvalid)?;
    let (credential, header, oauth_tokens) =
        prepare_server_account_authorization(state, &account, credential, None)
            .await
            .map_err(|_| AuthorizationFailure::Prepare)?;
    let (_, stored_fence) = state
        .store
        .account_refresh_scope(&fence.account_id)
        .map_err(|_| AuthorizationFailure::Stale)?;
    if &stored_fence != fence {
        return Err(AuthorizationFailure::Stale);
    }
    Ok(PreparedAuthorization {
        credential,
        header,
        oauth_tokens,
    })
}

pub(in crate::jobs) async fn request_authorization(
    state: &Arc<AppState>,
    fence: &AccountRefreshFence,
) -> Result<PreparedAuthorization, AuthorizationFailure> {
    let authorization_read = state
        .refresh
        .request(&fence.identity(), RefreshKind::Auth)
        .await
        .map_err(|error| match error {
            zenith_relay_core::scheduler::refresh::service::RefreshWaitError::Stale => {
                AuthorizationFailure::Stale
            }
            _ => AuthorizationFailure::Prepare,
        })?;
    let prepared = match authorization_read.as_ref() {
        Ok(RefreshRead::Authorization(Ok(prepared))) => Ok((**prepared).clone()),
        Ok(RefreshRead::Authorization(Err(error))) => Err(*error),
        _ => Err(AuthorizationFailure::Prepare),
    }?;
    // The shared read could have finished just before an operator changed the
    // login/proxy. Do not start provider HTTP with that obsolete preparation.
    let (_, stored_fence) = state
        .store
        .account_refresh_scope(&fence.account_id)
        .map_err(|_| AuthorizationFailure::Stale)?;
    if &stored_fence != fence {
        return Err(AuthorizationFailure::Stale);
    }
    Ok(prepared)
}

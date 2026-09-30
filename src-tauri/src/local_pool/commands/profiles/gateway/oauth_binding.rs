use super::super::*;

pub(super) async fn resolve_gateway_oauth_binding(
    state: &DesktopState,
    request: GatewayOAuthBindingRequest<'_>,
    profile_dir: &std::path::Path,
) -> LocalResult<Option<(String, PreparedAccountCredentials)>> {
    if request == GatewayOAuthBindingRequest::Disabled {
        return Ok(None);
    }
    let requested_account_id = match request {
        GatewayOAuthBindingRequest::Account(account_id) => Some(account_id),
        GatewayOAuthBindingRequest::Disabled | GatewayOAuthBindingRequest::Automatic => None,
    };
    let preferred_account_id = match requested_account_id {
        Some(account_id) => Some(account_id.to_string()),
        None => codex::active_managed_account_id(profile_dir, &state.profile_backup_root())?,
    };
    let automatic = requested_account_id.is_none();
    let mut candidates = {
        let store = state.store()?;
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let mut candidates = Vec::new();
        for account in store.accounts() {
            let explicitly_requested = requested_account_id == Some(account.account.id.as_str());
            if !account.account.enabled
                || !account.account.in_pool
                || account.account.draining
                || account.account.auth_state
                    != zenith_relay_core::accounts::AccountAuthState::Active
                || !candidate_health(&account.account).is_eligible()
                || !matches!(
                    account.account.auth_mode,
                    zenith_relay_core::accounts::AccountAuthMode::OAuth
                        | zenith_relay_core::accounts::AccountAuthMode::ImportedToken
                )
            {
                continue;
            }
            if credentials
                .load(&account.account.id)
                .map_err(|error| {
                    LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
                })?
                .is_some()
            {
                let Some(remaining) = profile_quota_rank(
                    candidate_quota_with_stale_after(
                        &account.account.quota,
                        super::super::super::current_time_ms(),
                        QUOTA_STALE_AFTER_MS,
                    ),
                    explicitly_requested,
                ) else {
                    continue;
                };
                candidates.push((account.account.id.clone(), remaining));
            }
        }
        candidates
    };
    prioritize_account_candidates(&mut candidates, preferred_account_id.as_deref(), automatic);
    if requested_account_id.is_some()
        && candidates
            .first()
            .is_none_or(|(candidate, _)| Some(candidate.as_str()) != requested_account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "selected OAuth account is not available to the local pool",
        ));
    }

    let mut last_error = None;
    for (account_id, _) in candidates {
        match prepare_account_credentials(state, &account_id).await {
            Ok(prepared)
                if prepared.tokens().refresh_token().is_some()
                    && prepared.tokens().id_token().is_some() =>
            {
                return Ok(Some((account_id, prepared)));
            }
            Ok(_) => {
                last_error = Some(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth binding requires refresh, identity, and account tokens",
                ));
            }
            Err(error) => last_error = Some(error),
        }
        if requested_account_id.is_some() {
            break;
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

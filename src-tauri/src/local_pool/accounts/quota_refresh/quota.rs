use super::*;
use crate::local_pool::{models::GatewaySettings, state::AccountCredentialFacts};
use std::collections::{BTreeSet, HashMap};
use zenith_relay_core::providers::chatgpt::OAuthClientKind;

pub(in crate::local_pool) fn quota_refresh_source(
    settings: &GatewaySettings,
    accounts: &[LocalAccountRecord],
    credentials: &HashMap<String, Option<AccountCredentialFacts>>,
    account_id: &str,
) -> String {
    let own = accounts
        .iter()
        .find(|account| account.account.id == account_id);
    let siwc = credentials
        .get(account_id)
        .and_then(Option::as_ref)
        .is_some_and(|facts| facts.oauth_client_kind == OAuthClientKind::Siwc);
    let Some(group) = settings.account_route_group(account_id).filter(|_| {
        siwc && own.is_some_and(|account| {
            account.recovery.is_none() && !account.account.auth_state.requires_fresh_login()
        })
    }) else {
        return account_id.to_string();
    };
    let score = |account: &LocalAccountRecord| {
        (
            account.account.quota.error.is_none(),
            account
                .account
                .quota
                .primary
                .iter()
                .chain(account.account.quota.secondary.iter())
                .map(|window| window.observed_at_ms)
                .min()
                .unwrap_or(0),
        )
    };
    accounts
        .iter()
        .filter(|account| {
            account.account.id != account_id
                && group.account_ids.contains(&account.account.id)
                && account.recovery.is_none()
                && account.account.is_automatic_quota_monitoring_eligible()
                && !account.account.draining
                && credentials
                    .get(&account.account.id)
                    .and_then(Option::as_ref)
                    .is_some_and(|facts| {
                        matches!(
                            facts.oauth_client_kind,
                            OAuthClientKind::Codex | OAuthClientKind::ExcelBps
                        ) && (facts.has_oauth || facts.agent_identity)
                    })
        })
        .reduce(|current, next| {
            if score(next) > score(current) {
                next
            } else {
                current
            }
        })
        .map_or_else(
            || account_id.to_string(),
            |source| source.account.id.clone(),
        )
}

pub(in crate::local_pool::accounts) fn quota_refresh_targets(
    state: &DesktopState,
    account_ids: &[String],
) -> LocalResult<Vec<String>> {
    let store = state.store()?;
    let facts = state.cached_account_credential_facts(store.accounts())?;
    let mut seen = BTreeSet::new();
    Ok(account_ids
        .iter()
        .map(|id| quota_refresh_source(store.gateway(), store.accounts(), &facts, id))
        .filter(|id| seen.insert(id.clone()))
        .collect())
}

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
    let quota_result = refresh_account_quota_once(state, &account_id).await;
    match quota_result {
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
    let target = quota_refresh_targets(state, &[account_id.to_string()])?
        .into_iter()
        .next()
        .unwrap_or_else(|| account_id.to_string());
    refresh_account_quota_once(state, &target).await
}

pub(crate) async fn prepare_account_credentials(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<PreparedAccountCredentials> {
    let recovery_required = state
        .store()?
        .account(account_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?
        .recovery
        .is_some();
    if recovery_required {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "account requires local recovery before use",
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
        CodexOAuthClient::new_with_proxy_for_kind(stored.oauth_client_kind(), proxy.as_ref())
            .map_err(LocalPoolError::invalid_state)?,
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
            let rejected_tokens = stored.to_token_set().map_err(credential_local_error)?;
            mark_access_only_reauthentication(state, account_id, &rejected_tokens).await?;
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "account access token expired and cannot be refreshed",
            ));
        }
        Err(error) => {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to prepare account credentials: {error}"),
            ));
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
    super::credentials::sync_account_profile_bindings(
        state,
        account_id,
        &prepared.tokens,
        &provider_account_id,
    )?;
    Ok(PreparedAccountCredentials {
        oauth_client_kind: current_credentials.oauth_client_kind(),
        tokens: prepared.tokens,
        provider_account_id,
        proxy,
    })
}

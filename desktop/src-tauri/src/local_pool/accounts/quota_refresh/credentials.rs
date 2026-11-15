use super::*;

mod failure;

pub(in crate::local_pool::accounts) use failure::{
    is_credential_refresh_error_code, persist_authorization_failure,
    persisted_token_generation_is_newer, token_set_is_newer,
};

pub(crate) fn sync_account_profile_bindings(
    state: &DesktopState,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> LocalResult<()> {
    let stored = CredentialStore::from_backend(NativeSecretBackend)
        .require(account_id)
        .map_err(credential_local_error)?;
    if stored.oauth_client_kind() != zenith_relay_core::providers::chatgpt::OAuthClientKind::Codex {
        return Ok(());
    }
    codex::sync_account_bindings(
        &state.profile_backup_root(),
        account_id,
        tokens,
        provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        tokens,
        provider_account_id,
    )?;
    Ok(())
}

pub(in crate::local_pool::accounts) fn reconcile_registered_account_record(
    state: &DesktopState,
    account_id: &str,
    expected_tokens: &TokenSet,
    authoritative_tokens: &TokenSet,
    authoritative_auth_state: AccountAuthState,
) -> LocalResult<()> {
    let (mut store, mut account) = super::load_stored_account(state, account_id)?;
    if persisted_token_generation_is_newer(
        account.account.token_generation,
        account.account.token_updated_at_ms,
        expected_tokens,
    ) || account.account.auth_state != AccountAuthState::Active
    {
        return Ok(());
    }
    account.account.token_generation = authoritative_tokens.generation();
    account.account.token_updated_at_ms = Some(authoritative_tokens.issued_at_ms());
    account.account.auth_state = authoritative_auth_state;
    if authoritative_auth_state == AccountAuthState::Active
        && account
            .account
            .last_error_code
            .as_deref()
            .is_some_and(is_credential_refresh_error_code)
    {
        account.account.last_error_code = None;
    }
    store.upsert_account(account)
}

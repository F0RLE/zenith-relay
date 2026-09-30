use super::super::{
    credential_item_error, find_existing_account, proxy_item_error, ImportItemError,
    ImportedCredentialMaterial, ItemResult,
};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::proxy::effective_proxy_config;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::models::{GatewaySettings, LocalAccountRecord};
use crate::local_pool::state::DesktopState;
use uuid::Uuid;
use zenith_relay_core::error_codes;
use zenith_relay_core::ProxyConfig;

pub(super) struct ReconciledImportCredentials {
    pub(super) credentials: StoredCodexCredentials,
    pub(super) old_credential: Option<StoredCodexCredentials>,
    pub(super) existing_account: Option<LocalAccountRecord>,
    pub(super) preserved_refresh_token: bool,
    pub(super) subscription_active_until_ms: Option<u64>,
    pub(super) proxy: Option<ProxyConfig>,
    pub(super) provider_account_id: String,
    pub(super) identity_is_registered: bool,
}

/// Match imported material to a local account and keep secrets the import did
/// not replace.
pub(super) fn reconcile_imported_credentials(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    settings: &GatewaySettings,
    issued_at_ms: u64,
    mut material: ImportedCredentialMaterial,
) -> ItemResult<ReconciledImportCredentials> {
    let provider_account_id = material.provider_account_id.as_deref().ok_or_else(|| {
        ImportItemError::new(
            error_codes::PROVIDER_ACCOUNT_ID_MISSING,
            "ChatGPT account id is missing from imported credentials",
        )
    })?;
    let existing_account = find_existing_account(
        state,
        credential_store,
        provider_account_id,
        material.provider_user_id.as_deref(),
        material.email.as_deref(),
    )?;
    let local_account_id = existing_account
        .as_ref()
        .map(|account| account.account.id.clone())
        .unwrap_or_else(|| format!("account_{}", Uuid::new_v4().simple()));
    let old_credential = credential_store
        .load(&local_account_id)
        .map_err(credential_item_error)?;
    let preserved_refresh_token = material.refresh_token.is_none()
        && old_credential
            .as_ref()
            .and_then(StoredCodexCredentials::refresh_token)
            .is_some();
    if preserved_refresh_token {
        material.refresh_token = old_credential
            .as_ref()
            .and_then(StoredCodexCredentials::refresh_token)
            .map(str::to_string);
    }
    let generation = old_credential
        .as_ref()
        .map(StoredCodexCredentials::generation)
        .into_iter()
        .chain(
            existing_account
                .as_ref()
                .map(|account| account.account.token_generation),
        )
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let subscription_active_until_ms = material.subscription_active_until_ms;
    let mut credentials = material.into_stored(&local_account_id, issued_at_ms, generation)?;
    if let Some(previous) = old_credential.as_ref() {
        credentials = credentials.fill_missing_login_from(previous);
    }
    if let Some(proxy_url) = old_credential
        .as_ref()
        .and_then(StoredCodexCredentials::proxy_url)
    {
        credentials = credentials
            .with_proxy_url(Some(proxy_url.to_string()))
            .map_err(credential_item_error)?;
    }
    let proxy = effective_proxy_config(settings, &credentials).map_err(proxy_item_error)?;
    let provider_account_id = credentials
        .provider_account_id()
        .map(str::to_string)
        .ok_or_else(|| {
            ImportItemError::new(
                error_codes::PROVIDER_ACCOUNT_ID_MISSING,
                "ChatGPT account id is missing from imported credentials",
            )
        })?;
    let identity_is_registered = credentials
        .agent_identity()
        .is_none_or(|agent| agent.task_id().is_some());
    Ok(ReconciledImportCredentials {
        credentials,
        old_credential,
        existing_account,
        preserved_refresh_token,
        subscription_active_until_ms,
        proxy,
        provider_account_id,
        identity_is_registered,
    })
}

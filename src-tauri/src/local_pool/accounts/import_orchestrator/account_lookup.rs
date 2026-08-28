use super::{
    credential_item_error, credential_local_error, provider_identity_key, ImportItemError,
    ItemResult,
};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::{records, NativeSecretBackend};
use crate::local_pool::error::Result as LocalResult;
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::DesktopState;
use std::collections::HashMap;

pub(in crate::local_pool::accounts) fn existing_identity_index(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
) -> LocalResult<HashMap<String, String>> {
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let mut existing = HashMap::new();
    for account_id in account_ids {
        let Some(credentials) = credential_store
            .load(&account_id)
            .map_err(credential_local_error)?
        else {
            continue;
        };
        let Some(provider_account_id) = credentials.provider_account_id() else {
            continue;
        };
        existing
            .entry(provider_identity_key(
                provider_account_id,
                credentials.provider_user_id(),
                credentials.email(),
            ))
            .or_insert(account_id);
    }
    Ok(existing)
}

pub(in crate::local_pool::accounts) fn find_existing_account(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    provider_account_id: &str,
    provider_user_id: Option<&str>,
    email: Option<&str>,
) -> ItemResult<Option<LocalAccountRecord>> {
    let accounts = state
        .store()
        .map_err(|_| ImportItemError::new("account_store_failed", "account store is unavailable"))?
        .accounts()
        .to_vec();
    let target = records::identity_hash(provider_account_id, provider_user_id, email);
    let direct = accounts
        .iter()
        .filter(|account| {
            account.account.source_id == records::CODEX_SOURCE_ID
                && account.account.identity.identity_hash == target
        })
        .cloned()
        .collect::<Vec<_>>();
    if direct.len() > 1 {
        return Err(ImportItemError::recovery(
            "multiple local accounts have the same ChatGPT identity",
        ));
    }
    if let Some(account) = direct.into_iter().next() {
        return Ok(Some(account));
    }
    let mut matching = Vec::new();
    for account in accounts {
        if account.account.source_id != records::CODEX_SOURCE_ID {
            continue;
        }
        let Some(credentials) = credential_store
            .load(&account.account.id)
            .map_err(credential_item_error)?
        else {
            continue;
        };
        let Some(account_id) = credentials.provider_account_id() else {
            continue;
        };
        if records::identity_hash(
            account_id,
            credentials.provider_user_id(),
            credentials.email(),
        ) == target
        {
            matching.push(account);
        }
    }
    if matching.len() > 1 {
        return Err(ImportItemError::recovery(
            "multiple local accounts have the same ChatGPT identity",
        ));
    }
    Ok(matching.pop())
}

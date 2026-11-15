use super::ProxyPool;
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::import_session::{SecretBackend, SecretBackendError};
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::store::secret_store;
use crate::storage_paths::StoragePaths;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::path::Path;
use zenith_relay_core::{
    providers::chatgpt::{OAuthClientKind, bearer_authorization, probe_subscription_authorization},
    scheduler::refresh::http::ManagementHttpScope,
    unix_time_ms,
};

#[cfg(test)]
mod tests;

struct Selection {
    record: LocalAccountRecord,
    credentials: StoredCodexCredentials,
    proxy_id: String,
    proxy_url: String,
}

pub(super) async fn compare(root: &Path) -> Result<Value, &'static str> {
    let selected = select(root)?;
    let tokens = selected
        .credentials
        .to_token_set()
        .map_err(|_| "credential_invalid")?;
    let authorization =
        bearer_authorization(tokens.access_token()).map_err(|_| "credential_invalid")?;
    let provider_account_id = selected
        .credentials
        .provider_account_id()
        .ok_or("account_identity_missing")?
        .to_owned();
    let proxy = zenith_relay_core::ProxyConfig::parse(&selected.proxy_url)
        .map_err(|_| "proxy_configuration_invalid")?;
    let checked_root = root.to_owned();
    let scope = ManagementHttpScope::checked(move || still_current(&checked_root, &selected));
    match probe_subscription_authorization(&proxy, scope, authorization, &provider_account_id).await
    {
        Ok(observations) => Ok(json!({
            "status":if observations.len() == 2 {"complete"} else {"deferred"},
            "accountCheckAccepted":true,
            "oauthClientKind":"Codex", "requestRoute":"existing_assigned_proxy",
            "sameAccountQuery":true, "generationTested":false,
            "workingConfigurationChanged":false, "observations":observations,
        })),
        Err(failure) => Ok(json!({
            "status":"deferred", "accountCheckAccepted":failure.endpoint_stage().map(|_| false),
            "code":failure.code, "httpStatus":failure.http_status(),
            "stage":failure.endpoint_stage(), "cfRay":failure.cf_ray(),
            "responseMitigation":failure.response_mitigation(),
            "retryAfterMs":failure.retry_after_ms(),
            "generationTested":false, "workingConfigurationChanged":false,
        })),
    }
}

fn eligible(record: &LocalAccountRecord) -> bool {
    record.recovery.is_none()
        && record.account.is_automatic_quota_monitoring_eligible()
        && !record.account.draining
}

fn select(root: &Path) -> Result<Selection, &'static str> {
    let vault = StoragePaths::from_root(root).vault_root();
    let pool = ProxyPool::load_read_only(&vault).map_err(|_| "proxy_pool_unavailable")?;
    for record in load_records(root)?.into_iter().filter(eligible) {
        let Some(proxy_id) = pool.proxy_id_for_account(&record.account.id) else {
            continue;
        };
        let credentials = load_credentials(root, &record.account.id)?;
        if credentials.oauth_client_kind() != OAuthClientKind::Codex || !credentials.has_oauth() {
            continue;
        }
        let proxy_url = pool
            .stored_url(&proxy_id)
            .map_err(|_| "proxy_configuration_invalid")?;
        if credentials.bypass_common_proxy() || credentials.proxy_url() != Some(proxy_url.as_str())
        {
            continue;
        }
        let tokens = credentials
            .to_token_set()
            .map_err(|_| "credential_invalid")?;
        if !tokens.is_access_usable(unix_time_ms(), 60_000) {
            return Err("account_access_needs_refresh");
        }
        if record.account.token_generation != tokens.generation() {
            return Err("account_credentials_changed");
        }
        return Ok(Selection {
            record,
            credentials,
            proxy_id,
            proxy_url,
        });
    }
    Err("monitorable_codex_account_with_assigned_proxy_required")
}

fn still_current(root: &Path, expected: &Selection) -> bool {
    let Ok(records) = load_records(root) else {
        return false;
    };
    let Some(current) = records
        .iter()
        .find(|record| record.account.id == expected.record.account.id)
    else {
        return false;
    };
    if !eligible(current) || !same_registration(current, &expected.record) {
        return false;
    }
    let Ok(credentials) = load_credentials(root, &current.account.id) else {
        return false;
    };
    if !credentials.matches_snapshot(&expected.credentials)
        || !credentials
            .to_token_set()
            .is_ok_and(|tokens| tokens.is_access_usable(unix_time_ms(), 60_000))
    {
        return false;
    }
    let Ok(pool) = ProxyPool::load_read_only(&StoragePaths::from_root(root).vault_root()) else {
        return false;
    };
    pool.proxy_id_for_account(&current.account.id).as_deref() == Some(expected.proxy_id.as_str())
        && pool
            .stored_url(&expected.proxy_id)
            .is_ok_and(|url| url == expected.proxy_url)
}

fn same_registration(current: &LocalAccountRecord, expected: &LocalAccountRecord) -> bool {
    current.account.created_at_ms == expected.account.created_at_ms
        && current.account.identity == expected.account.identity
        && current.account.source_id == expected.account.source_id
        && current.account.auth_mode == expected.account.auth_mode
        && current.account.auth_state == expected.account.auth_state
        && current.account.in_pool == expected.account.in_pool
        && current.account.secret_refs == expected.account.secret_refs
        && current.account.token_generation == expected.account.token_generation
        && current.account.token_updated_at_ms == expected.account.token_updated_at_ms
}

fn load_credentials(root: &Path, account_id: &str) -> Result<StoredCodexCredentials, &'static str> {
    CredentialStore::from_backend(DiagnosticSecrets(root))
        .require(account_id)
        .map_err(|_| "credential_unavailable")
}

struct DiagnosticSecrets<'a>(&'a Path);

impl SecretBackend for DiagnosticSecrets<'_> {
    fn save(&self, _: &str, _: &str) -> Result<(), SecretBackendError> {
        Err(SecretBackendError)
    }

    fn load(&self, reference: &str) -> Result<Option<String>, SecretBackendError> {
        secret_store::load_read_only(&StoragePaths::from_root(self.0).vault_root(), reference)
            .map_err(|_| SecretBackendError)
    }

    fn delete(&self, _: &str) -> Result<(), SecretBackendError> {
        Err(SecretBackendError)
    }
}

fn load_records(root: &Path) -> Result<Vec<LocalAccountRecord>, &'static str> {
    let paths = StoragePaths::from_root(root);
    for directory in [root.to_owned(), paths.data_root(), paths.database_root()] {
        let metadata = std::fs::symlink_metadata(directory).map_err(|_| "data_root_unavailable")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("data_root_unsafe");
        }
    }
    let database = paths.database_file();
    let metadata = std::fs::symlink_metadata(&database).map_err(|_| "database_unavailable")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("database_unsafe");
    }
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "database_unavailable")?;
    account_rows(&connection)?
        .into_iter()
        .map(|row| serde_json::from_str(&row).map_err(|_| "account_state_invalid"))
        .collect()
}

fn account_rows(connection: &Connection) -> Result<Vec<String>, &'static str> {
    let mut statement = connection.prepare(
        "SELECT COALESCE(items.value_json, base.value) FROM app_state AS state,
         json_each(state.value_json) AS base
         LEFT JOIN app_state_items AS items ON items.state_key='accounts' AND items.position=base.key
         WHERE state.key='accounts' ORDER BY base.key"
    ).map_err(|_| "account_state_unavailable")?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|_| "account_state_unavailable")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "account_state_invalid")
}

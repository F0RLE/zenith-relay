use crate::{
    codex_config::lock_codex_profile,
    files::{atomic_write, escape_json_string},
    local_pool::{
        error::{ErrorCode, LocalPoolError, Result},
        store::secret_store,
    },
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};
use toml_edit::{value, DocumentMut, Item, Table};
use zenith_relay_core::{accounts::TokenSet, DefaultServiceTier, CODEX_RELAY_CATALOG_HASH};
#[cfg(test)]
use zenith_relay_core::{codex_catalog_entry_is_compatible, routed_codex_catalog_entry};

mod account;
mod catalog;
mod catalog_state;
mod config;
mod local;
mod transaction;

use catalog_state::{
    apply_model_catalog_change, backup_path, invalidate_models_cache, local_backup,
    managed_model_catalog_path, reconcile_pending_catalog_state,
    remove_managed_model_catalog_if_unchanged, rollback_model_catalog_change,
    valid_managed_model_catalog,
};
use config::*;
use transaction::{
    io_error, io_error_at, merge_rollbacks, profile_changed_at, profile_restore_blocked,
    read_optional_bytes, remove_if_unchanged, replace_if_unchanged, replace_with_snapshot,
    restore_snapshot_if_unchanged, rollback_file, snapshot_text, with_rollback,
};

const PROVIDER_ID: &str = "zenith_relay_local";
const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
const MODEL_CATALOG_FILE: &str = "codex-model-catalog.json";
const MODELS_CACHE_FILE: &str = "models_cache.json";
const GLOBAL_STATE_FILE: &str = ".codex-global-state.json";
const DESKTOP_DEFAULT_SERVICE_TIER_KEY: &str = "default-service-tier";
const PERSISTED_ATOM_STATE_KEY: &str = "electron-persisted-atom-state";
const SERVICE_TIER_CHANGED_KEY: &str = "has-user-changed-service-tier";
const BACKUP_SECRET_REF: &str = "profile:codex:default:previous_auth";
const ACCOUNT_BACKUP_PREFIX: &str = "codex-account-";
const MAX_MANAGED_TOKEN_BYTES: usize = 64 * 1024;

/// Keep paths written into Codex config/backup metadata compatible with
/// consumers that do not understand Win32 extended-path prefixes.
pub(super) fn portable_path_string(path: &Path) -> String {
    let value = path.to_string_lossy();
    portable_path_value(&value)
}

pub(super) fn portable_path_value(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else {
        value.strip_prefix("\\\\?\\").unwrap_or(value).to_owned()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileBackup {
    version: u32,
    previous_model_provider: Option<String>,
    #[serde(default)]
    previous_model_catalog_json: Option<String>,
    #[serde(default)]
    previous_model_reasoning_effort: Option<String>,
    #[serde(default)]
    previous_auth_hash: Option<String>,
    previous_auth_secret_ref: Option<String>,
    #[serde(default)]
    managed_key_id: String,
    managed_key_hash: String,
    managed_base_url: String,
    #[serde(default)]
    bound_oauth_account_id: Option<String>,
    #[serde(default)]
    managed_oauth_access_hash: Option<String>,
    #[serde(default)]
    managed_bearer_in_config: bool,
    #[serde(default)]
    managed_supports_websockets: Option<bool>,
    #[serde(default)]
    managed_model_reasoning_effort_cleared: bool,
    #[serde(default)]
    managed_model_reasoning_effort: Option<String>,
    #[serde(default)]
    managed_model_catalog_path: Option<String>,
    #[serde(default)]
    managed_model_catalog_hash: Option<String>,
    #[serde(default)]
    managed_model_catalog_pending_hash: Option<String>,
    #[serde(default)]
    managed_model_catalog_pending_remove: bool,
    #[serde(default)]
    attach_pending: bool,
    #[serde(default)]
    restore_pending: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountProfileBackup {
    version: u32,
    profile_dir: String,
    previous_model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_openai_base_url: Option<String>,
    previous_auth_secret_ref: Option<String>,
    managed_account_id: String,
    managed_access_hash: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileCredentialKind {
    OAuthAccount,
    ApiKey,
    LocalGateway,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBinding {
    pub profile_dir: String,
    pub credential_kind: ProfileCredentialKind,
    pub credential_id: String,
    pub bound_oauth_account_id: Option<String>,
    pub active: bool,
}

/// Exact user-owned profile state held by a manually requested snapshot.
/// Payloads are stored in the OS secret store, never in snapshot metadata.
pub(super) struct UserProfileSnapshot {
    pub config: Option<String>,
    pub auth: Option<String>,
}

pub(crate) struct BoundOAuthProfile<'a> {
    pub account_id: &'a str,
    pub tokens: &'a TokenSet,
    pub provider_account_id: &'a str,
}

struct LocalAttachOptions<'a> {
    bound_oauth: Option<BoundOAuthProfile<'a>>,
    catalog_json: Option<&'a str>,
    supports_websockets: bool,
}

impl<'a> Default for LocalAttachOptions<'a> {
    fn default() -> Self {
        Self {
            bound_oauth: None,
            catalog_json: None,
            supports_websockets: true,
        }
    }
}

pub(crate) struct ManagedAccountTokenUpdate {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
}

impl fmt::Debug for ManagedAccountTokenUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedAccountTokenUpdate")
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

pub fn attach(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions::default(),
        &OsSecretBackend,
    )
}

pub fn attach_with_catalog(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    catalog_json: &str,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions {
            catalog_json: Some(catalog_json),
            ..LocalAttachOptions::default()
        },
        &OsSecretBackend,
    )
}

pub fn attach_with_catalog_and_websockets(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    catalog_json: &str,
    supports_websockets: bool,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions {
            catalog_json: Some(catalog_json),
            supports_websockets,
            ..LocalAttachOptions::default()
        },
        &OsSecretBackend,
    )
}

#[cfg(test)]
pub(crate) fn direct_source_model_catalog(
    codex_home: &Path,
    source_models: &[String],
) -> Result<Option<String>> {
    catalog::direct_source_model_catalog_with_manifest(codex_home, source_models, None)
}

pub(crate) fn direct_source_model_catalog_with_manifest(
    codex_home: &Path,
    source_models: &[String],
    source_manifest: Option<&Value>,
) -> Result<Option<String>> {
    catalog::direct_source_model_catalog_with_manifest(codex_home, source_models, source_manifest)
}

pub(crate) struct OAuthAttachOptions<'a> {
    pub catalog_json: &'a str,
    pub bound_oauth: BoundOAuthProfile<'a>,
    pub supports_websockets: bool,
}

pub(crate) fn attach_with_oauth_and_options(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    options: OAuthAttachOptions<'_>,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions {
            bound_oauth: Some(options.bound_oauth),
            catalog_json: Some(options.catalog_json),
            supports_websockets: options.supports_websockets,
        },
        &OsSecretBackend,
    )
}

pub fn restore(codex_home: &Path, backup_root: &Path) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        account::restore_account_locked(codex_home, backup_root, &OsSecretBackend)?;
        return Ok(());
    }
    local::restore_local_locked(codex_home, backup_root, &OsSecretBackend)
}

pub fn set_local_gateway_websockets(
    codex_home: &Path,
    backup_root: &Path,
    enabled: bool,
) -> Result<()> {
    set_local_gateway_websockets_with_previous(codex_home, backup_root, enabled).map(|_| ())
}

/// Updates the managed profile and returns the previous provider setting when
/// the profile was managed. Callers that persist a second copy of this state
/// can use the returned value to restore the profile if that later write fails.
pub fn set_local_gateway_websockets_with_previous(
    codex_home: &Path,
    backup_root: &Path,
    enabled: bool,
) -> Result<Option<bool>> {
    let _profile_guard = lock_codex_profile();
    if !codex_home.exists() {
        return Ok(None);
    }
    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let backup_path = backup_path(backup_root);
    let original_config = read_optional_bytes(&config_path)?;
    let Some(config_text) = snapshot_text(&original_config, &config_path)? else {
        return Ok(None);
    };
    let original_backup = read_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_backup_snapshot(&original_backup, &backup_path)? else {
        return Ok(None);
    };
    let mut document = parse_config(config_text)?;
    if !managed_config_matches(&document, &backup) {
        return Ok(None);
    }
    let previous_enabled = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .and_then(Item::as_table_like)
        .and_then(|provider| provider.get("supports_websockets"))
        .and_then(Item::as_bool)
        // A managed provider predates this field in some profiles. The
        // current Codex contract treats the omitted field as enabled.
        .or(Some(true));
    if document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .and_then(Item::as_table_like)
        .and_then(|provider| provider.get("supports_websockets"))
        .and_then(Item::as_bool)
        == Some(enabled)
        && backup.managed_supports_websockets == Some(enabled)
    {
        return Ok(previous_enabled);
    }
    if !set_managed_websockets(&mut document, enabled) {
        return Ok(previous_enabled);
    }
    let next_config = document.to_string();
    backup.managed_supports_websockets = Some(enabled);
    let next_backup = serialize_backup(&backup)?;
    if next_config != config_text {
        replace_if_unchanged(&config_path, &original_config, &next_config)?;
    }
    if let Err(error) = replace_if_unchanged(&backup_path, &original_backup, &next_backup) {
        let rollback = rollback_file(&config_path, &next_config, &original_config);
        return Err(with_rollback(error, rollback));
    }
    Ok(previous_enabled)
}

pub fn sync_default_service_tier(
    codex_home: &Path,
    default_service_tier: DefaultServiceTier,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    fs::create_dir_all(codex_home).map_err(io_error)?;
    let config_path = codex_home.join(CONFIG_FILE);
    let state_path = codex_home.join(GLOBAL_STATE_FILE);
    let original_config = read_optional_bytes(&config_path)?;
    let original_state = read_optional_bytes(&state_path)?;

    let mut document =
        parse_config(snapshot_text(&original_config, &config_path)?.unwrap_or_default())?;
    match default_service_tier {
        DefaultServiceTier::Standard => {
            if let Some(desktop) = document.get_mut("desktop") {
                desktop
                    .as_table_mut()
                    .ok_or_else(|| {
                        LocalPoolError::new(
                            ErrorCode::InvalidState,
                            "Codex desktop settings must be a table",
                        )
                    })?
                    .remove(DESKTOP_DEFAULT_SERVICE_TIER_KEY);
            }
        }
        DefaultServiceTier::Fast => {
            if document.get("desktop").is_none() {
                document["desktop"] = Item::Table(Table::new());
            }
            let desktop = document["desktop"].as_table_mut().ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "Codex desktop settings must be a table",
                )
            })?;
            desktop[DESKTOP_DEFAULT_SERVICE_TIER_KEY] = value("priority");
        }
    }
    let next_config = document.to_string();

    let mut state = match snapshot_text(&original_state, &state_path)? {
        Some(content) => serde_json::from_str::<Value>(content).map_err(|error| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("Codex global state is not valid JSON: {error}"),
            )
        })?,
        None => Value::Object(Default::default()),
    };
    let state = state.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex global state must be a JSON object",
        )
    })?;
    let persisted = state
        .entry(PERSISTED_ATOM_STATE_KEY.to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !persisted.is_object() {
        *persisted = Value::Object(Default::default());
    }
    let persisted = persisted
        .as_object_mut()
        .expect("persisted atom state was normalized to an object");
    persisted.insert(
        DESKTOP_DEFAULT_SERVICE_TIER_KEY.to_string(),
        match default_service_tier {
            DefaultServiceTier::Standard => Value::Null,
            DefaultServiceTier::Fast => Value::String("priority".to_string()),
        },
    );
    persisted.insert(SERVICE_TIER_CHANGED_KEY.to_string(), Value::Bool(true));
    let next_state = serde_json::to_string(state).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("Codex global state could not be serialized: {error}"),
        )
    })?;

    let config_changed = original_config
        .as_deref()
        .map_or(!next_config.is_empty(), |current| {
            current != next_config.as_bytes()
        });
    if config_changed {
        replace_if_unchanged(&config_path, &original_config, &next_config)?;
    }
    if original_state.as_deref() != Some(next_state.as_bytes()) {
        if let Err(error) = replace_if_unchanged(&state_path, &original_state, &next_state) {
            return Err(if config_changed {
                with_rollback(
                    error,
                    rollback_file(&config_path, &next_config, &original_config),
                )
            } else {
                error
            });
        }
    }
    Ok(())
}

pub fn attach_account(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<ProfileBinding> {
    switch_to_account_with(
        codex_home,
        backup_root,
        account_id,
        tokens,
        provider_account_id,
        &OsSecretBackend,
    )
}

pub fn restore_account_profile(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<Option<ProfileBinding>> {
    let _profile_guard = lock_codex_profile();
    account::restore_account_locked(codex_home, backup_root, &OsSecretBackend)
}

pub(super) fn snapshot_user_profile(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<UserProfileSnapshot> {
    snapshot_user_profile_with(codex_home, backup_root, &OsSecretBackend)
}

fn snapshot_user_profile_with(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<UserProfileSnapshot> {
    let _profile_guard = lock_codex_profile();
    fs::create_dir_all(codex_home).map_err(io_error)?;
    ensure_single_profile_backup(codex_home, backup_root)?;
    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let config = read_optional_bytes(&config_path)?;
    let auth = read_optional_bytes(&auth_path)?;
    let mut document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;

    if let Some(path) = account_backup_for_profile(&profile_dir, backup_root)? {
        let backup_bytes = read_optional_bytes(&path)?;
        let backup = parse_account_backup_snapshot(&backup_bytes, &path)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT account profile backup disappeared while creating a snapshot",
            )
        })?;
        if account_managed_config_matches(&document) {
            restore_account_config(&mut document, &backup);
            let auth =
                if account_auth_matches_snapshot(&auth, &auth_path, &backup.managed_access_hash)? {
                    previous_auth_snapshot(backup.previous_auth_secret_ref.as_deref(), secrets)?
                } else {
                    snapshot_text(&auth, &auth_path)?.map(str::to_string)
                };
            return Ok(UserProfileSnapshot {
                config: Some(document.to_string()),
                auth,
            });
        }
    } else if let Some(backup) = local_backup(codex_home, backup_root)? {
        if managed_config_matches(&document, &backup) {
            let model_catalog = model_catalog_to_restore(&document, &backup);
            let current_model_reasoning_effort = root_model_reasoning_effort(&document);
            restore_local_config(
                &mut document,
                &backup,
                model_catalog.as_deref(),
                current_model_reasoning_effort.as_deref(),
            );
            let auth = if managed_auth_matches_snapshot(&auth, &auth_path, &backup)? {
                previous_auth_snapshot(backup.previous_auth_secret_ref.as_deref(), secrets)?
            } else {
                snapshot_text(&auth, &auth_path)?.map(str::to_string)
            };
            return Ok(UserProfileSnapshot {
                config: Some(document.to_string()),
                auth,
            });
        }
        if external_provider_took_over(&document, &backup) {
            remove_managed_provider(&mut document);
            return Ok(UserProfileSnapshot {
                config: Some(document.to_string()),
                auth: snapshot_text(&auth, &auth_path)?.map(str::to_string),
            });
        }
    }

    Ok(UserProfileSnapshot {
        config: snapshot_text(&config, &config_path)?.map(str::to_string),
        auth: snapshot_text(&auth, &auth_path)?.map(str::to_string),
    })
}

pub(super) fn restore_full_user_profile_snapshot(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
) -> Result<()> {
    restore_user_profile_snapshot_full_with(codex_home, backup_root, snapshot, &OsSecretBackend)
}

fn restore_user_profile_snapshot_full_with(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    fs::create_dir_all(codex_home).map_err(io_error)?;
    ensure_single_profile_backup(codex_home, backup_root)?;
    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let original_config = read_optional_bytes(&config_path)?;
    let original_auth = read_optional_bytes(&auth_path)?;

    replace_with_snapshot(&config_path, &original_config, snapshot.config.as_deref())?;
    let target_config = snapshot
        .config
        .as_ref()
        .map(|value| value.as_bytes().to_vec());
    if let Err(error) = replace_with_snapshot(&auth_path, &original_auth, snapshot.auth.as_deref())
    {
        return Err(with_rollback(
            error,
            restore_snapshot_if_unchanged(&config_path, &target_config, &original_config),
        ));
    }
    let target_auth = snapshot
        .auth
        .as_ref()
        .map(|value| value.as_bytes().to_vec());
    if let Err(error) = discard_managed_binding_locked(&profile_dir, backup_root, secrets) {
        let auth_rollback = restore_snapshot_if_unchanged(&auth_path, &target_auth, &original_auth);
        let config_rollback =
            restore_snapshot_if_unchanged(&config_path, &target_config, &original_config);
        return Err(with_rollback(
            error,
            merge_rollbacks(auth_rollback, config_rollback),
        ));
    }
    Ok(())
}

pub fn credential_kind(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<Option<ProfileCredentialKind>> {
    let _profile_guard = lock_codex_profile();
    credential_kind_locked(codex_home, backup_root)
}

pub(crate) fn active_managed_account_id(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<Option<String>> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    if let Some(path) = account_backup_for_profile(codex_home, backup_root)? {
        let snapshot = read_optional_bytes(&path)?;
        return Ok(parse_account_backup_snapshot(&snapshot, &path)?
            .map(|backup| backup.managed_account_id));
    }
    Ok(local_backup(codex_home, backup_root)?.and_then(|backup| backup.bound_oauth_account_id))
}

pub fn profile_bindings(codex_home: &Path, backup_root: &Path) -> Result<Vec<ProfileBinding>> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    let mut bindings = account_bindings(backup_root)?;
    for binding in &mut bindings {
        let profile_dir = Path::new(&binding.profile_dir);
        let backup_path = account_backup_path(backup_root, profile_dir);
        let backup_content =
            fs::read_to_string(&backup_path).map_err(|error| io_error_at(&backup_path, error))?;
        let backup = parse_account_backup(&backup_content, &backup_path)?;
        let config_path = profile_dir.join(CONFIG_FILE);
        let config = read_optional_bytes(&config_path)?;
        let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
        let auth_path = profile_dir.join(AUTH_FILE);
        let auth = read_optional_bytes(&auth_path)?;
        binding.active = account_managed_config_matches(&document)
            && account_auth_matches_snapshot(&auth, &auth_path, &backup.managed_access_hash)?;
    }
    if let Some(backup) = local_backup(codex_home, backup_root)? {
        let profile_dir = canonical_profile_dir(codex_home)?;
        let config_path = profile_dir.join(CONFIG_FILE);
        let config = read_optional_bytes(&config_path)?;
        let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
        let auth_path = profile_dir.join(AUTH_FILE);
        let auth = read_optional_bytes(&auth_path)?;
        let active = managed_config_matches(&document, &backup)
            && managed_auth_matches_snapshot(&auth, &auth_path, &backup)?;
        bindings.push(ProfileBinding {
            profile_dir: profile_dir.to_string_lossy().into_owned(),
            credential_kind: ProfileCredentialKind::LocalGateway,
            credential_id: if backup.managed_key_id.is_empty() {
                "local_gateway".to_string()
            } else {
                backup.managed_key_id
            },
            bound_oauth_account_id: backup.bound_oauth_account_id,
            active,
        });
    } else if codex_home.exists() {
        let profile_dir = canonical_profile_dir(codex_home)?;
        let config_path = profile_dir.join(CONFIG_FILE);
        let config = read_optional_bytes(&config_path)?;
        let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
        if document_has_provider(&document) {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "managed ChatGPT provider exists without an automatic backup",
            ));
        }
    }
    bindings.sort_by(|left, right| left.profile_dir.cmp(&right.profile_dir));
    Ok(bindings)
}

pub fn account_bindings(backup_root: &Path) -> Result<Vec<ProfileBinding>> {
    if !backup_root.exists() {
        return Ok(Vec::new());
    }
    let mut bindings = Vec::new();
    for entry in fs::read_dir(backup_root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(ACCOUNT_BACKUP_PREFIX) || !name.ends_with(".json") {
            continue;
        }
        let path = entry.path();
        let content = fs::read_to_string(&path).map_err(|error| io_error_at(&path, error))?;
        let backup = parse_account_backup(&content, &path)?;
        bindings.push(binding_from_backup(&backup, false));
    }
    bindings.sort_by(|left, right| left.profile_dir.cmp(&right.profile_dir));
    Ok(bindings)
}

pub(crate) fn managed_account_token_update(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    current_access_token: &str,
    provider_account_id: &str,
) -> Result<Option<ManagedAccountTokenUpdate>> {
    let _profile_guard = lock_codex_profile();
    let mut update = None;

    if backup_root.exists() {
        for entry in fs::read_dir(backup_root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(ACCOUNT_BACKUP_PREFIX) || !name.ends_with(".json") {
                continue;
            }
            let backup_path = entry.path();
            let content = fs::read_to_string(&backup_path)
                .map_err(|error| io_error_at(&backup_path, error))?;
            let backup = parse_account_backup(&content, &backup_path)?;
            if backup.managed_account_id != account_id {
                continue;
            }
            merge_managed_token_update(
                &mut update,
                read_managed_account_token_update(
                    Path::new(&backup.profile_dir),
                    current_access_token,
                    provider_account_id,
                )?,
            )?;
        }
    }

    if let Some(backup) = local_backup(codex_home, backup_root)? {
        if backup.bound_oauth_account_id.as_deref() == Some(account_id) {
            merge_managed_token_update(
                &mut update,
                read_managed_account_token_update(
                    codex_home,
                    current_access_token,
                    provider_account_id,
                )?,
            )?;
        }
    }

    Ok(update)
}

fn read_managed_account_token_update(
    profile_dir: &Path,
    current_access_token: &str,
    provider_account_id: &str,
) -> Result<Option<ManagedAccountTokenUpdate>> {
    if !profile_dir.exists() {
        return Ok(None);
    }
    let profile_dir = canonical_profile_dir(profile_dir)?;
    let auth_path = profile_dir.join(AUTH_FILE);
    let snapshot = read_optional_bytes(&auth_path)?;
    let Some(content) = snapshot_text(&snapshot, &auth_path)? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(None);
    };
    if auth_credential_kind(&value) != Some(ProfileCredentialKind::OAuthAccount) {
        return Ok(None);
    }
    let Some(tokens) = value.get("tokens").and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };
    if tokens
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some(provider_account_id)
    {
        return Ok(None);
    }
    let Some(access_token) = managed_token(tokens, "access_token") else {
        return Ok(None);
    };
    if access_token == current_access_token {
        return Ok(None);
    }
    let Some(refresh_token) = managed_token(tokens, "refresh_token") else {
        return Ok(None);
    };
    let id_token = managed_token(tokens, "id_token").map(str::to_string);
    Ok(Some(ManagedAccountTokenUpdate {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        id_token,
    }))
}

fn managed_token<'a>(
    tokens: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    tokens
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= MAX_MANAGED_TOKEN_BYTES
                && !value.bytes().any(|byte| byte.is_ascii_control())
        })
}

fn merge_managed_token_update(
    current: &mut Option<ManagedAccountTokenUpdate>,
    next: Option<ManagedAccountTokenUpdate>,
) -> Result<()> {
    let Some(next) = next else {
        return Ok(());
    };
    if current.as_ref().is_some_and(|current| {
        current.access_token != next.access_token
            || current.refresh_token != next.refresh_token
            || current.id_token != next.id_token
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed ChatGPT profiles contain conflicting token generations",
        ));
    }
    *current = Some(next);
    Ok(())
}

pub fn sync_account_bindings(
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<usize> {
    let _profile_guard = lock_codex_profile();
    let mut updated = 0;
    for binding in account_bindings(backup_root)? {
        if binding.credential_id != account_id {
            continue;
        }
        let profile_dir = PathBuf::from(&binding.profile_dir);
        if !profile_dir.exists() {
            continue;
        }
        updated += usize::from(account::sync_account_profile_with(
            &profile_dir,
            backup_root,
            tokens,
            provider_account_id,
        )?);
    }
    Ok(updated)
}

pub fn sync_local_gateway_binding(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<bool> {
    let _profile_guard = lock_codex_profile();
    let _ = local_backup(codex_home, backup_root)?;
    let backup_path = backup_path(backup_root);
    let backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(false);
    };
    if backup.bound_oauth_account_id.as_deref() != Some(account_id) {
        return Ok(false);
    }
    if tokens.id_token().is_none() {
        return Ok(false);
    }
    let next_hash = key_hash(tokens.access_token());
    if backup.managed_oauth_access_hash.as_deref() == Some(next_hash.as_str()) {
        return Ok(false);
    }

    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let config = read_optional_bytes(&config_path)?;
    let auth = read_optional_bytes(&auth_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    let auth_matches_previous = managed_auth_matches_snapshot(&auth, &auth_path, &backup)?;
    let auth_matches_next = account_auth_matches_snapshot(&auth, &auth_path, &next_hash)?;
    if !managed_config_matches(&document, &backup) || (!auth_matches_previous && !auth_matches_next)
    {
        return Ok(false);
    }

    backup.managed_oauth_access_hash = Some(next_hash);
    let updated_backup = serialize_backup(&backup)?;
    replace_if_unchanged(&backup_path, &backup_bytes, &updated_backup)?;
    if auth_matches_next {
        return Ok(true);
    }
    let updated_auth = account_auth_content(tokens, provider_account_id)?;
    if let Err(error) = replace_if_unchanged(&auth_path, &auth, &updated_auth) {
        return Err(with_rollback(
            error,
            rollback_file(&backup_path, &updated_backup, &backup_bytes),
        ));
    }
    Ok(true)
}

pub(crate) fn refresh_managed_model_catalog(
    codex_home: &Path,
    backup_root: &Path,
    catalog_json: &str,
) -> Result<bool> {
    let _profile_guard = lock_codex_profile();
    let _ = local_backup(codex_home, backup_root)?;
    let backup_path = backup_path(backup_root);
    let mut backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(false);
    };
    if backup.attach_pending || backup.restore_pending {
        return Err(profile_restore_blocked());
    }
    let catalog_path = managed_model_catalog_path(backup_root)?;
    let mut catalog_bytes = read_optional_bytes(&catalog_path)?;
    reconcile_pending_catalog_state(&backup_path, &mut backup_bytes, &mut backup, &catalog_bytes)?;
    catalog_bytes = read_optional_bytes(&catalog_path)?;
    if !valid_managed_model_catalog(&backup, &catalog_path, &catalog_bytes) {
        return Err(profile_restore_blocked());
    }
    if backup.managed_model_catalog_path.is_none() {
        return Ok(false);
    }

    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let config = read_optional_bytes(&config_path)?;
    let auth = read_optional_bytes(&auth_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    if !managed_config_matches(&document, &backup)
        || !managed_auth_matches_snapshot(&auth, &auth_path, &backup)?
    {
        return Ok(false);
    }

    let catalog = catalog::build_managed_model_catalog(
        codex_home,
        backup.previous_model_catalog_json.as_deref(),
        catalog_bytes.as_deref(),
        catalog_json,
    )?;
    if catalog_bytes.as_deref() == Some(catalog.as_bytes()) {
        return Ok(false);
    }

    let original_backup_bytes = backup_bytes.clone();
    backup.managed_model_catalog_pending_hash = Some(key_hash(&catalog));
    backup.managed_model_catalog_pending_remove = false;
    let pending_backup = serialize_backup(&backup)?;
    replace_if_unchanged(&backup_path, &backup_bytes, &pending_backup)?;
    backup_bytes = Some(pending_backup.as_bytes().to_vec());
    if let Err(error) =
        apply_model_catalog_change(&catalog_path, &catalog_bytes, Some(&catalog), true)
    {
        return Err(with_rollback(
            error,
            rollback_file(&backup_path, &pending_backup, &original_backup_bytes),
        ));
    }
    backup.managed_model_catalog_hash = backup.managed_model_catalog_pending_hash.take();
    backup.managed_model_catalog_pending_remove = false;
    let committed_backup = serialize_backup(&backup)?;
    replace_if_unchanged(&backup_path, &backup_bytes, &committed_backup)?;
    let _ = invalidate_models_cache(codex_home);
    Ok(true)
}

fn switch_to_local_with(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    options: LocalAttachOptions<'_>,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    let detached_account_backup = match account_backup_for_profile(codex_home, backup_root)? {
        Some(path) if external_account_provider_took_over(codex_home)? => {
            let bytes = read_optional_bytes(&path)?;
            let backup = parse_account_backup_snapshot(&bytes, &path)?.ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "ChatGPT account profile backup disappeared during the switch",
                )
            })?;
            remove_if_unchanged(&path, &bytes)?;
            Some((path, bytes, backup.previous_auth_secret_ref))
        }
        Some(_) => {
            account::restore_account_locked(codex_home, backup_root, secrets)?;
            None
        }
        None => None,
    };
    local::prepare_existing_local_binding_locked(codex_home, backup_root, secrets)?;
    if let Err(error) = local::attach_local_locked(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        options,
        secrets,
    ) {
        let rollback = detached_account_backup
            .as_ref()
            .map(|(path, bytes, _)| restore_snapshot_if_unchanged(path, &None, bytes))
            .unwrap_or(Ok(()));
        return Err(with_rollback(error, rollback));
    }
    if let Some((path, bytes, Some(secret_ref))) = detached_account_backup {
        if let Err(error) = secrets.delete(&secret_ref) {
            let profile_rollback = local::restore_local_locked(codex_home, backup_root, secrets);
            let backup_rollback = restore_snapshot_if_unchanged(&path, &None, &bytes);
            return Err(with_rollback(
                error,
                merge_rollbacks(profile_rollback, backup_rollback),
            ));
        }
    }
    let backup = local_backup(codex_home, backup_root)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT local gateway profile backup is missing after attach",
        )
    })?;
    Ok(ProfileBinding {
        profile_dir: canonical_profile_dir(codex_home)?
            .to_string_lossy()
            .into_owned(),
        credential_kind: ProfileCredentialKind::LocalGateway,
        credential_id: key_id.to_string(),
        bound_oauth_account_id: backup.bound_oauth_account_id,
        active: true,
    })
}

fn switch_to_account_with(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    if backup_path(backup_root).exists() {
        local::restore_local_locked(codex_home, backup_root, secrets)?;
    }
    account::attach_account_locked(
        codex_home,
        backup_root,
        account_id,
        tokens,
        provider_account_id,
        secrets,
    )
}

#[cfg(test)]
fn ensure_test_native_catalog(home: &Path) {
    let path = home.join(MODELS_CACHE_FILE);
    let has_compatible_native = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|value| value.get("models").and_then(Value::as_array).cloned())
        .is_some_and(|models| {
            models.iter().any(|model| {
                catalog::is_native_catalog_entry(model) && codex_catalog_entry_is_compatible(model)
            })
        });
    if has_compatible_native {
        return;
    }
    let mut entry = routed_codex_catalog_entry(None, "gpt-5.6-sol", 1, None);
    entry["slug"] = Value::String("gpt-5.6-sol".into());
    entry["display_name"] = Value::String("GPT-5.6 Sol".into());
    entry["description"] = Value::String("Native test model".into());
    entry["comp_hash"] = Value::String("official".into());
    entry["default_reasoning_level"] = Value::String("low".into());
    entry["supported_reasoning_levels"] = json!([
        {"effort": "low", "description": "Low"},
        {"effort": "medium", "description": "Medium"}
    ]);
    entry["input_modalities"] = json!(["text", "image"]);
    let _ = fs::write(
        path,
        serde_json::to_string_pretty(&json!({"models": [entry]})).unwrap(),
    );
}

#[cfg(test)]
fn attach_with(
    codex_home: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    ensure_test_native_catalog(codex_home);
    local::prepare_existing_local_binding_locked(codex_home, backup_root, secrets)?;
    local::attach_local_locked(
        codex_home,
        backup_root,
        "local_gateway",
        base_url,
        local_key,
        LocalAttachOptions::default(),
        secrets,
    )
}

#[cfg(test)]
fn attach_with_catalog_for_test(
    codex_home: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
    catalog_json: &str,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    ensure_test_native_catalog(codex_home);
    local::prepare_existing_local_binding_locked(codex_home, backup_root, secrets)?;
    local::attach_local_locked(
        codex_home,
        backup_root,
        "local_gateway",
        base_url,
        local_key,
        LocalAttachOptions {
            catalog_json: Some(catalog_json),
            ..LocalAttachOptions::default()
        },
        secrets,
    )
}

#[cfg(test)]
fn restore_with(codex_home: &Path, backup_root: &Path, secrets: &impl SecretBackend) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    local::restore_local_locked(codex_home, backup_root, secrets)
}

#[cfg(test)]
fn attach_account_with(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let _profile_guard = lock_codex_profile();
    account::attach_account_locked(
        codex_home,
        backup_root,
        account_id,
        tokens,
        provider_account_id,
        secrets,
    )
}

#[cfg(test)]
fn restore_account_with(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<Option<ProfileBinding>> {
    let _profile_guard = lock_codex_profile();
    account::restore_account_locked(codex_home, backup_root, secrets)
}

fn attach_account_config(document: &mut DocumentMut) {
    let model_catalog = root_model_catalog_json(document);
    restore_config(document, None, model_catalog.as_deref());
    document.remove("openai_base_url");
}

fn restore_account_config(document: &mut DocumentMut, backup: &AccountProfileBackup) {
    let model_catalog = root_model_catalog_json(document);
    restore_config(
        document,
        backup.previous_model_provider.as_deref(),
        model_catalog.as_deref(),
    );
    if let Some(base_url) = backup.previous_openai_base_url.as_deref() {
        document["openai_base_url"] = value(base_url);
    }
}

fn account_managed_config_matches(document: &DocumentMut) -> bool {
    matches!(
        root_model_provider(document).as_deref(),
        None | Some("openai")
    ) && !document_has_provider(document)
}

fn account_auth_content(tokens: &TokenSet, provider_account_id: &str) -> Result<String> {
    let mut token_values = serde_json::Map::new();
    token_values.insert(
        "access_token".into(),
        serde_json::Value::String(tokens.access_token().to_string()),
    );
    token_values.insert(
        "account_id".into(),
        serde_json::Value::String(provider_account_id.to_string()),
    );
    token_values.insert(
        "refresh_token".into(),
        serde_json::Value::String(tokens.refresh_token().unwrap_or_default().to_string()),
    );
    if let Some(id_token) = tokens.id_token() {
        token_values.insert(
            "id_token".into(),
            serde_json::Value::String(id_token.to_string()),
        );
    }
    let last_refresh = i64::try_from(tokens.issued_at_ms())
        .ok()
        .filter(|milliseconds| *milliseconds > 0)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "OPENAI_API_KEY": null,
        "last_refresh": last_refresh,
        "tokens": token_values,
    }))
    .map_err(LocalPoolError::invalid_state)?;
    Ok(format!("{content}\n"))
}

fn account_auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
    expected_hash: &str,
) -> Result<bool> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(false);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(false);
    };
    Ok(
        auth_credential_kind(&value) == Some(ProfileCredentialKind::OAuthAccount)
            && value
                .get("tokens")
                .and_then(|tokens| tokens.get("access_token"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|token| key_hash(token.trim()) == expected_hash),
    )
}

fn account_backup_for_profile(codex_home: &Path, backup_root: &Path) -> Result<Option<PathBuf>> {
    if !codex_home.exists() {
        return Ok(None);
    }
    let path = account_backup_path(backup_root, &canonical_profile_dir(codex_home)?);
    Ok(path.exists().then_some(path))
}

fn ensure_single_profile_backup(codex_home: &Path, backup_root: &Path) -> Result<()> {
    if backup_path(backup_root).exists()
        && account_backup_for_profile(codex_home, backup_root)?.is_some()
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT profile has conflicting local gateway and account backups",
        ));
    }
    Ok(())
}

fn credential_kind_locked(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<Option<ProfileCredentialKind>> {
    ensure_single_profile_backup(codex_home, backup_root)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        return Ok(Some(ProfileCredentialKind::OAuthAccount));
    }
    if backup_path(backup_root).exists() {
        local_backup(codex_home, backup_root)?;
        return Ok(Some(ProfileCredentialKind::LocalGateway));
    }
    let auth_path = codex_home.join(AUTH_FILE);
    let auth = read_optional_bytes(&auth_path)?;
    let Some(content) = snapshot_text(&auth, &auth_path)? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(None);
    };
    Ok(auth_credential_kind(&value))
}

fn auth_credential_kind(value: &serde_json::Value) -> Option<ProfileCredentialKind> {
    match value.get("auth_mode").and_then(serde_json::Value::as_str) {
        Some("chatgpt") => Some(ProfileCredentialKind::OAuthAccount),
        Some("apikey") => Some(ProfileCredentialKind::ApiKey),
        Some(_) => None,
        None if value
            .get("OPENAI_API_KEY")
            .and_then(serde_json::Value::as_str)
            .is_some() =>
        {
            Some(ProfileCredentialKind::ApiKey)
        }
        None if value
            .get("tokens")
            .and_then(serde_json::Value::as_object)
            .is_some() =>
        {
            Some(ProfileCredentialKind::OAuthAccount)
        }
        None => None,
    }
}

fn account_backup_path(backup_root: &Path, profile_dir: &Path) -> PathBuf {
    backup_root.join(format!(
        "{ACCOUNT_BACKUP_PREFIX}{}.json",
        key_hash(profile_dir.to_string_lossy().as_ref())
    ))
}

fn account_backup_secret_ref(profile_dir: &Path) -> String {
    format!(
        "profile:codex:{}:previous_auth",
        key_hash(profile_dir.to_string_lossy().as_ref())
    )
}

fn canonical_profile_dir(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|error| io_error_at(path, error))?;
    if !canonical.is_dir() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT profile path is not a directory",
        ));
    }
    Ok(canonical)
}

fn parse_account_backup_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
) -> Result<Option<AccountProfileBackup>> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(None);
    };
    parse_account_backup(content, path).map(Some)
}

fn parse_account_backup(content: &str, path: &Path) -> Result<AccountProfileBackup> {
    let backup: AccountProfileBackup = serde_json::from_str(content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "ChatGPT account profile backup is invalid at {}: {error}",
                path.display()
            ),
        )
    })?;
    if backup.version != 1
        || backup.profile_dir.trim().is_empty()
        || backup.managed_account_id.trim().is_empty()
        || backup.managed_access_hash.len() != 64
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT account profile backup has invalid metadata",
        ));
    }
    Ok(backup)
}

fn serialize_account_backup(backup: &AccountProfileBackup) -> Result<String> {
    let content = serde_json::to_string_pretty(backup).map_err(LocalPoolError::invalid_state)?;
    Ok(format!("{content}\n"))
}

fn binding_from_backup(backup: &AccountProfileBackup, active: bool) -> ProfileBinding {
    ProfileBinding {
        profile_dir: backup.profile_dir.clone(),
        credential_kind: ProfileCredentialKind::OAuthAccount,
        credential_id: backup.managed_account_id.clone(),
        bound_oauth_account_id: None,
        active,
    }
}

fn rollback_account_backup(
    created: bool,
    backup_path: &Path,
    attempted_content: &str,
    previous_snapshot: &Option<Vec<u8>>,
    backup: &AccountProfileBackup,
    secrets: &impl SecretBackend,
) -> Result<()> {
    rollback_file(backup_path, attempted_content, previous_snapshot)?;
    cleanup_created_account_backup_secret(created, backup, secrets)
}

fn cleanup_created_account_backup_secret(
    created: bool,
    backup: &AccountProfileBackup,
    secrets: &impl SecretBackend,
) -> Result<()> {
    if !created {
        return Ok(());
    }
    if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
        secrets.delete(secret_ref)?;
    }
    Ok(())
}

fn parse_backup_snapshot(snapshot: &Option<Vec<u8>>, path: &Path) -> Result<Option<ProfileBackup>> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(None);
    };
    serde_json::from_str(content).map(Some).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("ChatGPT profile backup is invalid: {error}"),
        )
    })
}

fn serialize_backup(backup: &ProfileBackup) -> Result<String> {
    let content = serde_json::to_string_pretty(backup).map_err(LocalPoolError::invalid_state)?;
    Ok(format!("{content}\n"))
}

fn rollback_backup(
    created: bool,
    backup_path: &Path,
    attempted_content: &str,
    previous_snapshot: &Option<Vec<u8>>,
    backup: &ProfileBackup,
    secrets: &impl SecretBackend,
) -> Result<()> {
    rollback_file(backup_path, attempted_content, previous_snapshot)?;
    cleanup_created_backup_secret(created, backup, secrets)
}

fn cleanup_created_backup_secret(
    created: bool,
    backup: &ProfileBackup,
    secrets: &impl SecretBackend,
) -> Result<()> {
    if !created {
        return Ok(());
    }
    if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
        secrets.delete(secret_ref)?;
    }
    Ok(())
}

fn restore_secret_snapshot(
    snapshot: &Option<(String, Option<String>)>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let Some((secret_ref, value)) = snapshot else {
        return Ok(());
    };
    match value {
        Some(value) => secrets.save(secret_ref, value),
        None => secrets.delete(secret_ref),
    }
}

fn previous_auth_snapshot(
    secret_ref: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<Option<String>> {
    secret_ref
        .map(|secret_ref| {
            secrets.load(secret_ref)?.ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "ChatGPT profile backup secret is missing",
                )
            })
        })
        .transpose()
}

fn discard_managed_binding_locked(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<()> {
    if let Some(path) = account_backup_for_profile(codex_home, backup_root)? {
        let bytes = read_optional_bytes(&path)?;
        let backup = parse_account_backup_snapshot(&bytes, &path)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT account profile backup disappeared during snapshot restore",
            )
        })?;
        return discard_backup(
            &path,
            &bytes,
            backup.previous_auth_secret_ref.as_deref(),
            secrets,
        );
    }
    let path = backup_path(backup_root);
    let bytes = read_optional_bytes(&path)?;
    let Some(backup) = parse_backup_snapshot(&bytes, &path)? else {
        return Ok(());
    };
    remove_managed_model_catalog_if_unchanged(&backup);
    if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
        secrets.delete(secret_ref)?;
    }
    remove_if_unchanged(&path, &bytes)?;
    Ok(())
}

fn discard_backup(
    path: &Path,
    bytes: &Option<Vec<u8>>,
    secret_ref: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    remove_if_unchanged(path, bytes)?;
    let Some(secret_ref) = secret_ref else {
        return Ok(());
    };
    if let Err(error) = secrets.delete(secret_ref) {
        return Err(with_rollback(
            error,
            restore_snapshot_if_unchanged(path, &None, bytes),
        ));
    }
    Ok(())
}

trait SecretBackend {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()>;
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;
    fn delete(&self, secret_ref: &str) -> Result<()>;
}

struct OsSecretBackend;

impl SecretBackend for OsSecretBackend {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        secret_store::save(secret_ref, value)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        secret_store::load(secret_ref)
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        secret_store::delete(secret_ref)
    }
}

#[cfg(test)]
mod tests;

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
use zenith_relay_core::{
    accounts::TokenSet, model_metadata::ModelMetadataCatalog, DefaultServiceTier,
    CODEX_RELAY_CATALOG_HASH,
};
#[cfg(test)]
use zenith_relay_core::{codex_catalog_entry_is_compatible, routed_codex_catalog_entry};

mod account;
mod catalog;
mod catalog_state;
mod config;

pub(super) use super::portable_path_value;
mod local;
mod projection;
mod switch_transaction;
mod transaction;

mod account_ops;

pub(crate) fn official_codex_ultra_models() -> std::collections::HashMap<String, serde_json::Value>
{
    catalog::bundled_codex_ultra_models()
}

mod backup;
mod bindings;
mod gateway_attach;
mod service_tier;
mod switch_flow;

pub use account_ops::{attach_account, attach_account_explicit, restore_account_profile};
pub(super) use account_ops::{restore_full_user_profile_snapshot, snapshot_user_profile};
use backup::{
    account_auth_content, account_auth_matches_snapshot, account_auth_matches_tokens,
    account_backup_for_profile, account_backup_path, account_backup_secret_ref,
    account_managed_config_matches, attach_account_config, auth_credential_kind,
    auth_snapshot_json, binding_from_backup, canonical_profile_dir, cleanup_account_attach_secrets,
    cleanup_created_backup_secret, credential_kind_locked, delete_backup_secrets, discard_backup,
    discard_managed_binding_locked, ensure_single_profile_backup, fill_missing_account_config,
    parse_account_backup, parse_account_backup_snapshot, parse_backup_snapshot,
    previous_auth_snapshot, restore_account_config, restore_secret_snapshot,
    rollback_account_backup, rollback_backup, serialize_account_backup, serialize_backup,
};
use bindings::managed_token;
pub use bindings::{
    account_bindings, credential_kind, profile_bindings, sync_account_bindings,
    sync_local_gateway_binding,
};
pub(crate) use bindings::{
    active_managed_account_id, managed_account_token_update, refresh_managed_model_catalog,
};
use catalog_state::{
    apply_model_catalog_change, backup_path, externally_changed_managed_model_catalog,
    invalidate_models_cache, local_backup, managed_model_catalog_path,
    reconcile_pending_catalog_state, remove_managed_model_catalog_if_unchanged,
    rollback_model_catalog_change, valid_managed_model_catalog,
};
use config::*;
#[cfg(test)]
use gateway_attach::set_local_gateway_websockets_with_backend;
pub use gateway_attach::{
    attach, attach_with_catalog, attach_with_catalog_and_websockets, restore,
    set_local_gateway_websockets_with_previous,
};
pub(crate) use gateway_attach::{
    attach_ready_api, attach_ready_api_explicit, attach_with_oauth_and_options,
    direct_source_model_catalog_with_capabilities, restore_ready_api,
};
#[cfg(test)]
pub(crate) use gateway_attach::{
    direct_source_model_catalog, direct_source_model_catalog_with_manifest,
};
pub use service_tier::sync_default_service_tier;
#[cfg(test)]
use switch_flow::{
    attach_account_with, attach_with, attach_with_catalog_for_test, ensure_test_native_catalog,
    restore_account_with, restore_with,
};
use switch_flow::{switch_to_account_with, switch_to_account_with_intent, switch_to_local_with};
use transaction::{
    io_error, io_error_at, merge_rollbacks, profile_changed_at, profile_restore_blocked,
    read_optional_bytes, remove_if_unchanged, replace_if_unchanged, replace_with_snapshot,
    restore_snapshot_if_unchanged, rollback_file, snapshot_text, with_rollback,
};

const PROVIDER_ID: &str = "zenith_relay_local";
const NATIVE_PROVIDER_ID: &str = "openai";
const READY_API_PROVIDER_ID: &str = "codex_local_access";
const LEGACY_READY_API_PROVIDER_ID: &str = "zenith";
const RELAY_PROVIDER_IDS: [&str; 3] = [
    PROVIDER_ID,
    READY_API_PROVIDER_ID,
    LEGACY_READY_API_PROVIDER_ID,
];
const READY_API_PROVIDER_NAME: &str = "OpenAI";
const LEGACY_READY_API_PROVIDER_NAME: &str = "Zenith";

fn default_managed_provider() -> String {
    PROVIDER_ID.to_owned()
}

impl ProfileBackup {
    fn credential_kind(&self) -> ProfileCredentialKind {
        if self.managed_provider_id == READY_API_PROVIDER_ID {
            ProfileCredentialKind::ApiKey
        } else {
            ProfileCredentialKind::LocalGateway
        }
    }
}
const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
const MODEL_CATALOG_FILE: &str = "codex-model-catalog.json";
const MODELS_CACHE_FILE: &str = "models_cache.json";
const GLOBAL_STATE_FILE: &str = ".codex-global-state.json";
const DESKTOP_DEFAULT_SERVICE_TIER_KEY: &str = "default-service-tier";
const DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY: &str = "show-ultra-in-model-picker-slider";
const TOP_LEVEL_SERVICE_TIER_KEY: &str = "service_tier";
// This is a legacy Codex storage key. Keep its exact name for compatibility;
// it does not indicate that Relay embeds or runs Electron.
const PERSISTED_ATOM_STATE_KEY: &str = "electron-persisted-atom-state";
const SERVICE_TIER_CHANGED_KEY: &str = "has-user-changed-service-tier";
const BACKUP_SECRET_REF: &str = "profile:codex:default:previous_auth";
const ACCOUNT_BACKUP_PREFIX: &str = "codex-account-";
const MAX_MANAGED_TOKEN_BYTES: usize = 64 * 1024;

/// Keep paths written into Codex config/backup metadata compatible with
/// consumers that do not understand Win32 extended-path prefixes.
pub(super) fn portable_path_string(path: &Path) -> String {
    let path_text = path.to_string_lossy();
    super::portable_path_value(&path_text)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileBackup {
    version: u32,
    #[serde(default = "default_managed_provider")]
    managed_provider_id: String,
    #[serde(default)]
    projection_secret_ref: Option<String>,
    previous_model_provider: Option<String>,
    #[serde(default)]
    previous_model_catalog_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_review_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_chatgpt_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_openai_base_url: Option<String>,
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
    /// True only when this attachment turned the Ultra picker switch on.
    /// Absent in older backups, which did not change the switch.
    #[serde(default)]
    managed_show_ultra_picker: bool,
    /// `None` means the key was absent. Codex treats that as off.
    #[serde(default)]
    previous_show_ultra_picker: Option<bool>,
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
    #[serde(default)]
    projection_secret_ref: Option<String>,
    profile_dir: String,
    previous_model_provider: Option<String>,
    /// The native Codex catalog must not be kept active while an OAuth
    /// account profile is attached. Older backups did not record this leaf;
    /// the projection secret remains the authoritative restore source for
    /// newly-created backups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_model_catalog_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_review_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_chatgpt_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_openai_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_model_reasoning_effort: Option<String>,
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
    provider_id: &'a str,
    bound_oauth: Option<BoundOAuthProfile<'a>>,
    catalog_json: Option<&'a str>,
    supports_websockets: bool,
    rebase_newer_login: bool,
}

impl<'a> Default for LocalAttachOptions<'a> {
    fn default() -> Self {
        Self {
            provider_id: PROVIDER_ID,
            bound_oauth: None,
            catalog_json: None,
            supports_websockets: true,
            rebase_newer_login: false,
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

pub(crate) struct OAuthAttachOptions<'a> {
    pub catalog_json: &'a str,
    pub bound_oauth: BoundOAuthProfile<'a>,
    pub supports_websockets: bool,
}

trait SecretBackend {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()>;
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;
    fn delete(&self, secret_ref: &str) -> Result<()>;
}

struct OsSecretBackend;

impl SecretBackend for OsSecretBackend {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
        secret_store::save(secret_ref, secret_value)
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

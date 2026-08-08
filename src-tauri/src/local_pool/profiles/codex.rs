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
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt, fs,
    path::{Path, PathBuf},
};
use toml_edit::{value, DocumentMut, Item, Table};
use zenith_relay_core::{
    accounts::TokenSet, canonicalize_model_ids, codex_catalog_entry_is_compatible,
    codex_model_display_name, codex_model_is_picker_eligible, decode_codex_model_alias,
    normalize_codex_catalog_priorities, normalize_native_codex_catalog_entry,
    normalize_upstream_codex_catalog_entry, routed_codex_catalog_entry, CODEX_RELAY_CATALOG_HASH,
};

const PROVIDER_ID: &str = "zenith_relay_local";
const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
const MODEL_CATALOG_FILE: &str = "codex-model-catalog.json";
const MODELS_CACHE_FILE: &str = "models_cache.json";
const BACKUP_SECRET_REF: &str = "profile:codex:default:previous_auth";
const ACCOUNT_BACKUP_PREFIX: &str = "codex-account-";
const MAX_MANAGED_TOKEN_BYTES: usize = 64 * 1024;
const MAX_MODEL_CATALOG_BYTES: usize = 512 * 1024;
const DIRECT_SOURCE_FALLBACK_PRIORITY: u64 = 1_000;

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
    managed_supports_websockets: bool,
    #[serde(default)]
    managed_model_reasoning_effort_cleared: bool,
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

pub(super) struct UserProfileSnapshot {
    pub config: Option<String>,
    pub auth: Option<String>,
}

#[derive(Clone, Copy)]
enum ManagedSnapshotScope {
    LocalGateway,
    OAuthAccount,
    NoBinding,
}

pub(crate) struct BoundOAuthProfile<'a> {
    pub account_id: &'a str,
    pub tokens: &'a TokenSet,
    pub provider_account_id: &'a str,
}

#[derive(Default)]
struct LocalAttachOptions<'a> {
    bound_oauth: Option<BoundOAuthProfile<'a>>,
    catalog_json: Option<&'a str>,
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

#[cfg(test)]
pub(crate) fn direct_source_model_catalog(
    codex_home: &Path,
    source_models: &[String],
) -> Result<Option<String>> {
    direct_source_model_catalog_with_manifest(codex_home, source_models, None)
}

pub(crate) fn direct_source_model_catalog_with_manifest(
    codex_home: &Path,
    source_models: &[String],
    source_manifest: Option<&Value>,
) -> Result<Option<String>> {
    let user_catalog_path = configured_model_catalog_path(codex_home)?;
    let template = collect_native_catalog_template(codex_home, user_catalog_path.as_deref(), None)?;
    // A catalog override is optional in Codex.  Relay should prefer a verified
    // native row when one is present, but must not make profile attachment
    // depend on a cache that it deliberately invalidates after catalog changes.
    let template = template.unwrap_or_default();
    // `model_provider` points to this selected source.  Native Codex rows are
    // useful only as a schema template here; advertising them would send their
    // requests to this source and produce a false model picker entry.
    let selected_models = source_models
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|model| is_direct_source_model(model) && codex_model_is_picker_eligible(model))
        .collect::<Vec<_>>();
    let selected_models = canonicalize_model_ids(selected_models);

    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for (index, model) in selected_models.into_iter().enumerate() {
        let normalized = model.to_ascii_lowercase();
        if !seen.insert(normalized) {
            continue;
        }
        let entry = direct_source_catalog_entry(
            &template,
            source_manifest.and_then(|manifest| source_catalog_entry(manifest, &model)),
            &model,
            DIRECT_SOURCE_FALLBACK_PRIORITY + index as u64,
        );
        if codex_catalog_entry_is_compatible(&entry) {
            models.push(entry);
        }
    }
    if models.is_empty() {
        return Ok(None);
    }
    Ok(Some(normalize_model_catalog_values(models)?))
}

fn is_direct_source_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && !model.chars().any(char::is_control)
        && !model.to_ascii_lowercase().starts_with("zenith/")
}

fn cached_native_catalog_models(codex_home: &Path) -> Vec<Value> {
    let Ok(content) = fs::read_to_string(codex_home.join(MODELS_CACHE_FILE)) else {
        return Vec::new();
    };
    let Ok(cache) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    cache
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| is_native_catalog_entry(entry))
        .filter(|entry| codex_catalog_entry_is_compatible(entry))
        .cloned()
        .collect()
}

fn is_native_catalog_entry(entry: &Value) -> bool {
    entry
        .get("slug")
        .and_then(Value::as_str)
        .is_some_and(|slug| {
            !slug.to_ascii_lowercase().starts_with("zenith/")
                && entry
                    .get("comp_hash")
                    .and_then(Value::as_str)
                    .is_none_or(|hash| hash != CODEX_RELAY_CATALOG_HASH)
        })
}

fn model_slug(entry: &Value) -> Option<&str> {
    entry.get("slug").and_then(Value::as_str)
}

fn catalog_entry_is_picker_eligible(entry: &Value) -> bool {
    model_slug(entry).is_some_and(|slug| {
        let model = decode_codex_model_alias(slug).unwrap_or_else(|| slug.to_string());
        codex_model_is_picker_eligible(&model)
    })
}

fn direct_source_catalog_entry(
    template: &serde_json::Map<String, Value>,
    source_entry: Option<&serde_json::Map<String, Value>>,
    model: &str,
    priority: u64,
) -> Value {
    let mut entry = source_entry
        .and_then(|source_entry| {
            normalize_upstream_codex_catalog_entry(source_entry, model, priority, None)
        })
        .unwrap_or_else(|| routed_codex_catalog_entry(Some(template), model, priority, None));
    entry["slug"] = Value::String(model.to_string());
    entry["display_name"] = Value::String(codex_model_display_name(model));
    entry["description"] = Value::String("Available through this API connection.".into());
    entry["comp_hash"] = Value::String(CODEX_RELAY_CATALOG_HASH.into());
    entry
}

fn source_catalog_entry<'a>(
    manifest: &'a Value,
    model: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    manifest
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find(|entry| {
            entry
                .get("slug")
                .and_then(Value::as_str)
                .is_some_and(|slug| slug.eq_ignore_ascii_case(model))
        })
        .or_else(|| {
            manifest
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .find(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id.eq_ignore_ascii_case(model))
                })
        })
}

fn collect_native_catalog_template(
    codex_home: &Path,
    user_catalog_path: Option<&str>,
    managed_catalog: Option<&[u8]>,
) -> Result<Option<serde_json::Map<String, Value>>> {
    let mut candidates = Vec::new();
    if let Some(path) = user_catalog_path {
        candidates.extend(read_catalog_file_models(codex_home, path)?);
    }
    candidates.extend(cached_native_catalog_models(codex_home));
    let managed_models = match managed_catalog {
        Some(content) => read_catalog_values(content, false)?,
        None => Vec::new(),
    };
    // Attaching Relay invalidates Codex's live cache after writing a verified
    // catalog. On a later refresh, the current managed catalog is therefore
    // the only remaining compatible schema template. It is never returned as
    // a native model: routed_codex_catalog_entry resets capability fields for
    // a plain upstream /v1/models row before it is advertised again.
    let managed_template = managed_models
        .iter()
        .filter(|entry| {
            codex_catalog_entry_is_compatible(entry) && catalog_entry_is_picker_eligible(entry)
        })
        .find_map(Value::as_object)
        .cloned();
    candidates.extend(managed_models);

    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for candidate in candidates {
        if !is_native_catalog_entry(&candidate) || !codex_catalog_entry_is_compatible(&candidate) {
            continue;
        }
        let Some(slug) = model_slug(&candidate) else {
            continue;
        };
        if seen.insert(slug.to_ascii_lowercase()) {
            models.push(candidate);
        }
    }
    let picker_template = |entry: &&Value| {
        catalog_entry_is_picker_eligible(entry)
            && entry.get("supported_in_api") != Some(&Value::Bool(false))
    };
    // Prefer an actual native entry over a namespaced user provider row.  The
    // latter remains a useful schema fallback when it is the only catalog
    // available, but must not override native client capabilities by default.
    let template = models
        .iter()
        .filter(|entry| picker_template(entry))
        .filter(|entry| model_slug(entry).is_some_and(|slug| !slug.contains('/')))
        .find_map(Value::as_object)
        .cloned()
        .or_else(|| {
            models
                .iter()
                .filter(|entry| picker_template(entry))
                .find_map(Value::as_object)
                .cloned()
        })
        .or(managed_template);
    Ok(template)
}

fn configured_model_catalog_path(codex_home: &Path) -> Result<Option<String>> {
    let config_path = codex_home.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    Ok(root_model_catalog_json(&document))
}

fn read_catalog_file_models(codex_home: &Path, configured_path: &str) -> Result<Vec<Value>> {
    let configured_path = Path::new(configured_path);
    let path = if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        codex_home.join(configured_path)
    };
    let content = fs::read(&path).map_err(|error| io_error_at(&path, error))?;
    read_catalog_values(&content, false)
}

fn read_catalog_values(content: &[u8], require_compatible: bool) -> Result<Vec<Value>> {
    if content.len() > MAX_MODEL_CATALOG_BYTES {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog exceeds 512 KiB",
        ));
    }
    let value: Value = serde_json::from_slice(content).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "ChatGPT model catalog is invalid")
    })?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty() && models.len() <= 4_096)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "ChatGPT model catalog has no usable models",
            )
        })?;
    let mut output = Vec::new();
    for model in models {
        if require_compatible && !codex_catalog_entry_is_compatible(model) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "ChatGPT model catalog contains incompatible model entries",
            ));
        }
        if !require_compatible || codex_catalog_entry_is_compatible(model) {
            output.push(model.clone());
        }
    }
    Ok(output)
}

fn normalize_model_catalog_values(models: Vec<Value>) -> Result<String> {
    if models.is_empty() || models.len() > 4_096 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog has no usable models",
        ));
    }
    let mut seen = HashSet::new();
    let mut models = models
        .into_iter()
        .filter(codex_catalog_entry_is_compatible)
        .filter(|model| {
            model_slug(model).is_some_and(|slug| seen.insert(slug.to_ascii_lowercase()))
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog has no compatible models",
        ));
    }
    normalize_codex_catalog_priorities(&mut models);
    serde_json::to_string_pretty(&json!({ "models": models }))
        .map(|content| format!("{content}\n"))
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))
}

fn build_managed_model_catalog(
    codex_home: &Path,
    user_catalog_path: Option<&str>,
    current_managed_catalog: Option<&[u8]>,
    relay_catalog_json: &str,
) -> Result<String> {
    let template =
        collect_native_catalog_template(codex_home, user_catalog_path, current_managed_catalog)?;
    let template = template.unwrap_or_default();
    let relay_models = read_catalog_values(relay_catalog_json.as_bytes(), false)?;
    // The managed provider is the Relay endpoint, so the catalog must contain
    // only models that its live pool exposes.  Native/user catalog rows remain
    // untouched in their original profile and only supply a compatible template.
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    let mut accepted = 0usize;
    for (index, relay_model) in relay_models.iter().enumerate() {
        let Some(slug) = model_slug(relay_model) else {
            continue;
        };
        // Direct-source catalogs keep the provider's bare slug for Codex, so
        // the alias prefix alone cannot distinguish them from native rows.
        // The Relay catalog marker is the ownership boundary here.
        let relay_managed = slug.to_ascii_lowercase().starts_with("zenith/")
            || relay_model
                .get("comp_hash")
                .and_then(Value::as_str)
                .is_some_and(|hash| hash == CODEX_RELAY_CATALOG_HASH);
        let model = if slug.to_ascii_lowercase().starts_with("zenith/") {
            let Some(model) = decode_codex_model_alias(slug) else {
                continue;
            };
            model
        } else {
            slug.to_string()
        };
        if !codex_model_is_picker_eligible(&model) {
            continue;
        }
        accepted += 1;
        let context_window = relay_model
            .get("context_window")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0);
        let priority = relay_model
            .get("priority")
            .and_then(Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(DIRECT_SOURCE_FALLBACK_PRIORITY + index as u64);
        // A Relay-owned row may have come from a real upstream Codex catalog.
        // Preserve its strictly validated capability data (including arbitrary
        // reasoning levels) instead of inheriting anything from the native
        // template. Bare rows without the Relay marker are native rows.
        let mut entry = relay_model
            .as_object()
            .and_then(|upstream| {
                if relay_managed {
                    normalize_upstream_codex_catalog_entry(
                        upstream,
                        &model,
                        priority,
                        context_window,
                    )
                } else {
                    normalize_native_codex_catalog_entry(upstream, &model, priority, context_window)
                }
            })
            .unwrap_or_else(|| {
                routed_codex_catalog_entry(Some(&template), &model, priority, context_window)
            });
        if !slug.to_ascii_lowercase().starts_with("zenith/") {
            entry["slug"] = Value::String(slug.to_string());
        }
        entry["comp_hash"] = Value::String(CODEX_RELAY_CATALOG_HASH.into());
        if let Some(display_name) = relay_model.get("display_name").and_then(Value::as_str) {
            entry["display_name"] = Value::String(display_name.to_string());
        }
        if let Some(description) = relay_model.get("description").and_then(Value::as_str) {
            entry["description"] = Value::String(description.to_string());
        }
        if let Some(slug) = model_slug(&entry) {
            if seen.insert(slug.to_ascii_lowercase()) {
                models.push(entry);
            }
        }
    }
    if accepted == 0 {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "pool has no compatible text models",
        ));
    }
    normalize_model_catalog_values(models)
}

pub(crate) fn attach_with_oauth_and_catalog(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    catalog_json: &str,
    bound_oauth: BoundOAuthProfile<'_>,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions {
            bound_oauth: Some(bound_oauth),
            catalog_json: Some(catalog_json),
        },
        &OsSecretBackend,
    )
}

pub fn restore(codex_home: &Path, backup_root: &Path) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        restore_account_locked(codex_home, backup_root, &OsSecretBackend)?;
        return Ok(());
    }
    restore_local_locked(codex_home, backup_root, &OsSecretBackend)
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
    restore_account_locked(codex_home, backup_root, &OsSecretBackend)
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
            restore_local_config(&mut document, &backup, model_catalog.as_deref());
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

pub(super) fn restore_user_profile_snapshot(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
) -> Result<()> {
    restore_user_profile_snapshot_managed_with(codex_home, backup_root, snapshot, &OsSecretBackend)
}

pub(super) fn restore_full_user_profile_snapshot(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
) -> Result<()> {
    restore_user_profile_snapshot_full_with(codex_home, backup_root, snapshot, &OsSecretBackend)
}

fn restore_user_profile_snapshot_managed_with(
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

    let current_config = snapshot_text(&original_config, &config_path)?;
    let document = parse_config(current_config.unwrap_or_default())?;
    validate_config_shape(&document)?;
    let scope = managed_snapshot_scope(
        &profile_dir,
        backup_root,
        &document,
        &original_auth,
        &auth_path,
    )?;
    let target_config =
        merge_managed_snapshot_config(current_config, snapshot.config.as_deref(), scope)?;
    let target_auth = merge_managed_snapshot_auth(
        snapshot_text(&original_auth, &auth_path)?,
        snapshot.auth.as_deref(),
    )?;
    let config_changed = target_config.as_deref().map(str::as_bytes) != original_config.as_deref();
    let auth_changed = target_auth.as_deref().map(str::as_bytes) != original_auth.as_deref();
    let attempted_config = target_config
        .as_deref()
        .map(|content| content.as_bytes().to_vec());
    let attempted_auth = target_auth
        .as_deref()
        .map(|content| content.as_bytes().to_vec());

    if config_changed {
        replace_with_snapshot(&config_path, &original_config, target_config.as_deref())?;
    }
    if auth_changed {
        if let Err(error) =
            replace_with_snapshot(&auth_path, &original_auth, target_auth.as_deref())
        {
            let rollback = if config_changed {
                restore_snapshot_if_unchanged(&config_path, &attempted_config, &original_config)
            } else {
                Ok(())
            };
            return Err(with_rollback(error, rollback));
        }
    }
    if let Err(error) = discard_managed_binding_locked(&profile_dir, backup_root, secrets) {
        let auth_rollback = if auth_changed {
            restore_snapshot_if_unchanged(&auth_path, &attempted_auth, &original_auth)
        } else {
            Ok(())
        };
        let config_rollback = if config_changed {
            restore_snapshot_if_unchanged(&config_path, &attempted_config, &original_config)
        } else {
            Ok(())
        };
        return Err(with_rollback(
            error,
            merge_rollbacks(auth_rollback, config_rollback),
        ));
    }
    Ok(())
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
    let current_hash = key_hash(current_access_token);
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
            if backup.managed_account_id != account_id || backup.managed_access_hash != current_hash
            {
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
        if backup.bound_oauth_account_id.as_deref() == Some(account_id)
            && backup.managed_oauth_access_hash.as_deref() == Some(current_hash.as_str())
        {
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
        updated += usize::from(sync_account_profile_with(
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

    let catalog = build_managed_model_catalog(
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
            restore_account_locked(codex_home, backup_root, secrets)?;
            None
        }
        None => None,
    };
    prepare_existing_local_binding_locked(codex_home, backup_root, secrets)?;
    if let Err(error) = attach_local_locked(
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
            let profile_rollback = restore_local_locked(codex_home, backup_root, secrets);
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
        restore_local_locked(codex_home, backup_root, secrets)?;
    }
    attach_account_locked(
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
                is_native_catalog_entry(model) && codex_catalog_entry_is_compatible(model)
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
    prepare_existing_local_binding_locked(codex_home, backup_root, secrets)?;
    attach_local_locked(
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
    prepare_existing_local_binding_locked(codex_home, backup_root, secrets)?;
    attach_local_locked(
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

fn prepare_existing_local_binding_locked(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _ = local_backup(codex_home, backup_root)?;
    let path = backup_path(backup_root);
    let bytes = read_optional_bytes(&path)?;
    let Some(mut backup) = parse_backup_snapshot(&bytes, &path)? else {
        return Ok(());
    };
    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    if external_provider_took_over(&document, &backup) {
        return Ok(());
    }
    if backup.previous_model_catalog_json.is_none()
        && backup.managed_model_catalog_path.is_none()
        && backup.managed_model_catalog_hash.is_none()
        && backup.managed_model_catalog_pending_hash.is_none()
        && managed_config_matches(&document, &backup)
    {
        if let Some(legacy_catalog) = root_model_catalog_json(&document) {
            backup.previous_model_catalog_json = Some(legacy_catalog);
            let updated = serialize_backup(&backup)?;
            replace_if_unchanged(&path, &bytes, &updated)?;
        }
    }
    restore_local_locked(codex_home, backup_root, secrets)
}

fn attach_local_locked(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    options: LocalAttachOptions<'_>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let key_id = key_id.trim();
    let local_key = local_key.trim();
    let base_url = base_url.trim_end_matches('/');
    let catalog_json = options.catalog_json;
    let bound_oauth = normalize_bound_oauth(options.bound_oauth)?;
    if key_id.is_empty() || local_key.is_empty() || base_url.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "profile credential ID, base URL, and credential are required",
        ));
    }
    fs::create_dir_all(codex_home).map_err(io_error)?;
    fs::create_dir_all(backup_root).map_err(io_error)?;
    let _ = local_backup(codex_home, backup_root)?;
    let catalog_path = managed_model_catalog_path(backup_root)?;
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let backup_path = backup_path(backup_root);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_backup_bytes = read_optional_bytes(&backup_path)?;
    let original_catalog_bytes = read_optional_bytes(&catalog_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let original_auth = snapshot_text(&original_auth_bytes, &auth_path)?;
    let mut document = parse_config(original_config)?;
    validate_config_shape(&document)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "ChatGPT profile is already attached to an OAuth account",
        ));
    }
    let existing_backup = parse_backup_snapshot(&original_backup_bytes, &backup_path)?;
    if existing_backup.as_ref().is_some_and(|backup| {
        !valid_managed_model_catalog(backup, &catalog_path, &original_catalog_bytes)
    }) {
        return Err(profile_restore_blocked());
    }
    let had_managed_catalog = existing_backup
        .as_ref()
        .and_then(|backup| backup.managed_model_catalog_path.as_deref())
        .is_some();

    if catalog_json.is_some() && !had_managed_catalog && original_catalog_bytes.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed ChatGPT model catalog exists without a profile backup",
        ));
    }

    if existing_backup.is_none() && document_has_provider(&document) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed ChatGPT provider exists without a profile backup",
        ));
    }
    let external_takeover = existing_backup
        .as_ref()
        .is_some_and(|backup| external_provider_took_over(&document, backup));
    if existing_backup.is_some()
        && !managed_config_matches(&document, existing_backup.as_ref().unwrap())
        && !external_takeover
    {
        return Err(profile_restore_blocked());
    }
    if let Some(backup) = existing_backup.as_ref() {
        if !external_takeover
            && !managed_auth_matches_snapshot(&original_auth_bytes, &auth_path, backup)?
            && !previous_auth_matches_snapshot(&original_auth_bytes, backup)
        {
            return Err(profile_restore_blocked());
        }
    }

    let user_catalog_path = if external_takeover {
        existing_backup
            .as_ref()
            .and_then(|backup| external_model_catalog(&document, backup))
    } else {
        existing_backup
            .as_ref()
            .and_then(|backup| {
                backup
                    .managed_model_catalog_path
                    .as_ref()
                    .and(backup.previous_model_catalog_json.as_ref())
                    .cloned()
            })
            .or_else(|| root_model_catalog_json(&document))
    };
    let catalog = catalog_json
        .map(|content| {
            build_managed_model_catalog(
                codex_home,
                user_catalog_path.as_deref(),
                had_managed_catalog
                    .then_some(original_catalog_bytes.as_deref())
                    .flatten(),
                content,
            )
        })
        .transpose()?;

    let created_backup = existing_backup.is_none();
    let mut backup = existing_backup.unwrap_or(ProfileBackup {
        version: 1,
        previous_model_provider: root_model_provider(&document),
        previous_model_catalog_json: root_model_catalog_json(&document),
        previous_model_reasoning_effort: root_model_reasoning_effort(&document),
        previous_auth_hash: original_auth_bytes.as_deref().map(bytes_hash),
        previous_auth_secret_ref: None,
        managed_key_id: String::new(),
        managed_key_hash: String::new(),
        managed_base_url: String::new(),
        bound_oauth_account_id: None,
        managed_oauth_access_hash: None,
        managed_bearer_in_config: false,
        managed_supports_websockets: false,
        managed_model_reasoning_effort_cleared: false,
        managed_model_catalog_path: None,
        managed_model_catalog_hash: None,
        managed_model_catalog_pending_hash: None,
        managed_model_catalog_pending_remove: false,
        attach_pending: false,
        restore_pending: false,
    });
    if !created_backup
        && backup.managed_model_catalog_path.is_none()
        && backup.managed_model_catalog_hash.is_none()
    {
        backup.previous_model_catalog_json = root_model_catalog_json(&document);
    }
    if created_backup || external_takeover || !backup.managed_model_reasoning_effort_cleared {
        backup.previous_model_reasoning_effort = root_model_reasoning_effort(&document);
    }
    backup.managed_model_reasoning_effort_cleared = true;
    let rebased_secret = if external_takeover {
        backup.previous_model_provider = root_model_provider(&document);
        backup.previous_model_catalog_json = external_model_catalog(&document, &backup);
        backup.previous_auth_hash = original_auth_bytes.as_deref().map(bytes_hash);
        let secret_ref = backup
            .previous_auth_secret_ref
            .clone()
            .unwrap_or_else(|| BACKUP_SECRET_REF.to_string());
        let previous_secret = secrets.load(&secret_ref)?;
        if let Some(previous_auth) = original_auth.filter(|value| !value.trim().is_empty()) {
            secrets.save(&secret_ref, previous_auth)?;
            backup.previous_auth_secret_ref = Some(secret_ref.clone());
        } else {
            secrets.delete(&secret_ref)?;
            backup.previous_auth_secret_ref = None;
        }
        Some((secret_ref, previous_secret))
    } else {
        None
    };
    if created_backup {
        if let Some(previous_auth) = original_auth.filter(|value| !value.trim().is_empty()) {
            secrets.save(BACKUP_SECRET_REF, previous_auth)?;
            backup.previous_auth_secret_ref = Some(BACKUP_SECRET_REF.to_string());
        }
    } else if backup.previous_auth_hash.is_none() {
        if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
            backup.previous_auth_hash = secrets
                .load(secret_ref)?
                .map(|content| bytes_hash(content.as_bytes()));
        }
    }
    backup.managed_key_id = key_id.to_string();
    backup.managed_key_hash = key_hash(local_key);
    backup.managed_base_url = base_url.to_string();
    let managed_oauth_access_hash = bound_oauth
        .as_ref()
        .filter(|oauth| oauth.tokens.id_token().is_some())
        .map(|oauth| key_hash(oauth.tokens.access_token()));
    let project_bound_oauth = managed_oauth_access_hash.is_some();
    backup.bound_oauth_account_id = bound_oauth
        .as_ref()
        .map(|oauth| oauth.account_id.to_string());
    backup.managed_oauth_access_hash = managed_oauth_access_hash;
    backup.managed_bearer_in_config = true;
    backup.managed_supports_websockets = false;
    let previous_managed_catalog_path = backup.managed_model_catalog_path.clone();
    let previous_managed_catalog_hash = backup.managed_model_catalog_hash.clone();
    backup.managed_model_catalog_path = if catalog.is_some() {
        Some(catalog_path.to_string_lossy().into_owned())
    } else {
        previous_managed_catalog_path
    };
    backup.managed_model_catalog_hash = previous_managed_catalog_hash;
    backup.managed_model_catalog_pending_hash = catalog.as_deref().map(key_hash);
    backup.managed_model_catalog_pending_remove =
        catalog.is_none() && backup.managed_model_catalog_path.is_some();
    backup.attach_pending = true;
    backup.restore_pending = false;
    let backup_content = match serialize_backup(&backup) {
        Ok(content) => content,
        Err(error) => {
            return Err(with_rollback(
                error,
                merge_rollbacks(
                    cleanup_created_backup_secret(created_backup, &backup, secrets),
                    restore_secret_snapshot(&rebased_secret, secrets),
                ),
            ));
        }
    };
    if let Err(error) = replace_if_unchanged(&backup_path, &original_backup_bytes, &backup_content)
    {
        return Err(with_rollback(
            error,
            merge_rollbacks(
                cleanup_created_backup_secret(created_backup, &backup, secrets),
                restore_secret_snapshot(&rebased_secret, secrets),
            ),
        ));
    }

    if let Err(error) = apply_model_catalog_change(
        &catalog_path,
        &original_catalog_bytes,
        catalog.as_deref(),
        had_managed_catalog,
    ) {
        return Err(with_rollback(
            error,
            merge_rollbacks(
                rollback_backup(
                    created_backup,
                    &backup_path,
                    &backup_content,
                    &original_backup_bytes,
                    &backup,
                    secrets,
                ),
                restore_secret_snapshot(&rebased_secret, secrets),
            ),
        ));
    }

    attach_config(
        &mut document,
        base_url,
        local_key,
        catalog
            .as_ref()
            .map(|_| catalog_path.to_string_lossy().into_owned())
            .as_deref(),
        backup.previous_model_catalog_json.as_deref(),
    );
    let managed_config = document.to_string();
    if let Err(error) = replace_if_unchanged(&config_path, &original_config_bytes, &managed_config)
    {
        return Err(with_rollback(
            error,
            merge_rollbacks(
                rollback_model_catalog_change(
                    &catalog_path,
                    catalog.as_deref(),
                    had_managed_catalog,
                    &original_catalog_bytes,
                ),
                merge_rollbacks(
                    rollback_backup(
                        created_backup,
                        &backup_path,
                        &backup_content,
                        &original_backup_bytes,
                        &backup,
                        secrets,
                    ),
                    restore_secret_snapshot(&rebased_secret, secrets),
                ),
            ),
        ));
    }
    let managed_auth = match bound_oauth {
        Some(oauth) if project_bound_oauth => {
            account_auth_content(oauth.tokens, oauth.provider_account_id)?
        }
        Some(_) | None => auth_content(local_key),
    };
    if let Err(error) = replace_if_unchanged(&auth_path, &original_auth_bytes, &managed_auth) {
        let config_rollback = rollback_file(&config_path, &managed_config, &original_config_bytes);
        let backup_rollback = merge_rollbacks(
            rollback_model_catalog_change(
                &catalog_path,
                catalog.as_deref(),
                had_managed_catalog,
                &original_catalog_bytes,
            ),
            merge_rollbacks(
                rollback_backup(
                    created_backup,
                    &backup_path,
                    &backup_content,
                    &original_backup_bytes,
                    &backup,
                    secrets,
                ),
                restore_secret_snapshot(&rebased_secret, secrets),
            ),
        );
        return Err(with_rollback(
            error,
            merge_rollbacks(config_rollback, backup_rollback),
        ));
    }
    let pending_backup_bytes = backup_content.as_bytes().to_vec();
    let mut committed_backup = backup.clone();
    committed_backup.managed_model_catalog_path = catalog
        .as_ref()
        .map(|_| catalog_path.to_string_lossy().into_owned());
    committed_backup.managed_model_catalog_hash = catalog.as_deref().map(key_hash);
    committed_backup.managed_model_catalog_pending_hash = None;
    committed_backup.managed_model_catalog_pending_remove = false;
    committed_backup.attach_pending = false;
    committed_backup.restore_pending = false;
    let committed_backup_content = serialize_backup(&committed_backup)?;
    replace_if_unchanged(
        &backup_path,
        &Some(pending_backup_bytes),
        &committed_backup_content,
    )?;
    if original_catalog_bytes.as_deref() != catalog.as_deref().map(str::as_bytes) {
        let _ = invalidate_models_cache(codex_home);
    }
    Ok(())
}

fn normalize_bound_oauth(
    bound_oauth: Option<BoundOAuthProfile<'_>>,
) -> Result<Option<BoundOAuthProfile<'_>>> {
    let Some(bound_oauth) = bound_oauth else {
        return Ok(None);
    };
    let account_id = bound_oauth.account_id.trim();
    let provider_account_id = bound_oauth.provider_account_id.trim();
    if account_id.is_empty()
        || account_id.chars().any(char::is_control)
        || provider_account_id.is_empty()
        || bound_oauth.tokens.refresh_token().is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OAuth binding requires active refresh and account tokens",
        ));
    }
    Ok(Some(BoundOAuthProfile {
        account_id,
        tokens: bound_oauth.tokens,
        provider_account_id,
    }))
}

#[cfg(test)]
fn restore_with(codex_home: &Path, backup_root: &Path, secrets: &impl SecretBackend) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    restore_local_locked(codex_home, backup_root, secrets)
}

fn restore_local_locked(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _ = local_backup(codex_home, backup_root)?;
    let backup_path = backup_path(backup_root);
    let mut backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(());
    };
    let catalog_path = managed_model_catalog_path(backup_root)?;
    let catalog_bytes = read_optional_bytes(&catalog_path)?;
    if !valid_managed_model_catalog(&backup, &catalog_path, &catalog_bytes) {
        return Err(profile_restore_blocked());
    }
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let mut document = parse_config(original_config)?;
    let config_matches_managed = managed_config_matches(&document, &backup);
    let config_matches_previous = previous_config_matches(&document, &backup);
    if !config_matches_managed && !config_matches_previous {
        return Err(profile_restore_blocked());
    }
    let previous_auth = match backup.previous_auth_secret_ref.as_deref() {
        Some(secret_ref) => secrets.load(secret_ref)?,
        None => None,
    };
    if backup.previous_auth_hash.is_none() {
        backup.previous_auth_hash = previous_auth
            .as_deref()
            .map(|content| bytes_hash(content.as_bytes()));
    }
    if let (Some(expected), Some(content)) = (
        backup.previous_auth_hash.as_deref(),
        previous_auth.as_deref(),
    ) {
        if bytes_hash(content.as_bytes()) != expected {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT profile backup secret does not match its integrity hash",
            ));
        }
    }
    let auth_matches_managed =
        managed_auth_matches_snapshot(&original_auth_bytes, &auth_path, &backup)?;
    let auth_matches_previous = previous_auth_matches_snapshot(&original_auth_bytes, &backup);
    if !auth_matches_managed && !auth_matches_previous {
        return Err(profile_restore_blocked());
    }
    if auth_matches_managed && backup.previous_auth_secret_ref.is_some() && previous_auth.is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT profile backup secret is missing",
        ));
    }
    if read_optional_bytes(&backup_path)? != backup_bytes {
        return Err(profile_changed_at(&backup_path));
    }
    if !backup.restore_pending {
        backup.restore_pending = true;
        let pending_backup = serialize_backup(&backup)?;
        replace_if_unchanged(&backup_path, &backup_bytes, &pending_backup)?;
        backup_bytes = Some(pending_backup.into_bytes());
    }

    let model_catalog = backup.previous_model_catalog_json.clone();
    restore_local_config(&mut document, &backup, model_catalog.as_deref());
    let restored_config = document.to_string();
    if original_config_bytes.as_deref() != Some(restored_config.as_bytes()) {
        replace_if_unchanged(&config_path, &original_config_bytes, &restored_config)?;
    }

    if !auth_matches_previous {
        match previous_auth.as_deref() {
            Some(previous_auth) => {
                replace_if_unchanged(&auth_path, &original_auth_bytes, previous_auth)?;
            }
            None => remove_if_unchanged(&auth_path, &original_auth_bytes)?,
        }
    }

    if catalog_bytes.is_some() {
        remove_if_unchanged(&catalog_path, &catalog_bytes)?;
    }
    if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
        secrets.delete(secret_ref)?;
    }
    if backup.managed_model_catalog_path.is_some() {
        let _ = invalidate_models_cache(codex_home);
    }
    remove_if_unchanged(&backup_path, &backup_bytes)?;
    Ok(())
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
    attach_account_locked(
        codex_home,
        backup_root,
        account_id,
        tokens,
        provider_account_id,
        secrets,
    )
}

fn attach_account_locked(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let account_id = account_id.trim();
    let provider_account_id = provider_account_id.trim();
    if account_id.is_empty()
        || account_id.chars().any(char::is_control)
        || tokens.access_token().trim().is_empty()
        || provider_account_id.is_empty()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account profile credentials are invalid",
        ));
    }
    fs::create_dir_all(codex_home).map_err(io_error)?;
    fs::create_dir_all(backup_root).map_err(io_error)?;
    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let backup_path = account_backup_path(backup_root, &profile_dir);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_backup_bytes = read_optional_bytes(&backup_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let original_auth = snapshot_text(&original_auth_bytes, &auth_path)?;
    let mut document = parse_config(original_config)?;
    validate_config_shape(&document)?;
    let existing_backup = parse_account_backup_snapshot(&original_backup_bytes, &backup_path)?;

    if existing_backup.is_none() && document_has_provider(&document) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "restore the local gateway profile before attaching an OAuth account",
        ));
    }
    if let Some(backup) = existing_backup.as_ref() {
        if backup.profile_dir != profile_dir.to_string_lossy()
            || !account_managed_config_matches(&document)
            || !account_auth_matches_snapshot(
                &original_auth_bytes,
                &auth_path,
                &backup.managed_access_hash,
            )?
        {
            return Err(profile_restore_blocked());
        }
    }

    let created_backup = existing_backup.is_none();
    let mut backup = existing_backup.unwrap_or(AccountProfileBackup {
        version: 1,
        profile_dir: profile_dir.to_string_lossy().into_owned(),
        previous_model_provider: root_model_provider(&document),
        previous_openai_base_url: root_openai_base_url(&document),
        previous_auth_secret_ref: None,
        managed_account_id: String::new(),
        managed_access_hash: String::new(),
    });
    if backup.previous_openai_base_url.is_none() {
        backup.previous_openai_base_url = root_openai_base_url(&document);
    }
    if created_backup {
        if let Some(previous_auth) = original_auth.filter(|value| !value.trim().is_empty()) {
            let secret_ref = account_backup_secret_ref(&profile_dir);
            secrets.save(&secret_ref, previous_auth)?;
            backup.previous_auth_secret_ref = Some(secret_ref);
        }
    }
    backup.managed_account_id = account_id.to_string();
    backup.managed_access_hash = key_hash(tokens.access_token());
    let backup_content = serialize_account_backup(&backup)?;
    if let Err(error) = replace_if_unchanged(&backup_path, &original_backup_bytes, &backup_content)
    {
        return Err(with_rollback(
            error,
            cleanup_created_account_backup_secret(created_backup, &backup, secrets),
        ));
    }

    attach_account_config(&mut document);
    let managed_config = document.to_string();
    if let Err(error) = replace_if_unchanged(&config_path, &original_config_bytes, &managed_config)
    {
        return Err(with_rollback(
            error,
            rollback_account_backup(
                created_backup,
                &backup_path,
                &backup_content,
                &original_backup_bytes,
                &backup,
                secrets,
            ),
        ));
    }
    let managed_auth = account_auth_content(tokens, provider_account_id)?;
    if let Err(error) = replace_if_unchanged(&auth_path, &original_auth_bytes, &managed_auth) {
        let config_rollback = rollback_file(&config_path, &managed_config, &original_config_bytes);
        let backup_rollback = rollback_account_backup(
            created_backup,
            &backup_path,
            &backup_content,
            &original_backup_bytes,
            &backup,
            secrets,
        );
        return Err(with_rollback(
            error,
            merge_rollbacks(config_rollback, backup_rollback),
        ));
    }
    Ok(binding_from_backup(&backup, true))
}

#[cfg(test)]
fn restore_account_with(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<Option<ProfileBinding>> {
    let _profile_guard = lock_codex_profile();
    restore_account_locked(codex_home, backup_root, secrets)
}

fn restore_account_locked(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<Option<ProfileBinding>> {
    let profile_dir = canonical_profile_dir(codex_home)?;
    let backup_path = account_backup_path(backup_root, &profile_dir);
    let backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(backup) = parse_account_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(None);
    };
    if backup.profile_dir != profile_dir.to_string_lossy() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT account profile backup points to another profile",
        ));
    }
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let mut document = parse_config(original_config)?;
    if !account_managed_config_matches(&document)
        || !account_auth_matches_snapshot(
            &original_auth_bytes,
            &auth_path,
            &backup.managed_access_hash,
        )?
    {
        return Err(profile_restore_blocked());
    }
    let previous_auth = match backup.previous_auth_secret_ref.as_deref() {
        Some(secret_ref) => Some(secrets.load(secret_ref)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT account profile backup secret is missing",
            )
        })?),
        None => None,
    };
    restore_account_config(&mut document, &backup);
    let restored_config = document.to_string();
    replace_if_unchanged(&config_path, &original_config_bytes, &restored_config)?;

    let restored_auth_bytes = previous_auth
        .as_ref()
        .map(|content| content.as_bytes().to_vec());
    let auth_result = match previous_auth.as_deref() {
        Some(previous_auth) => {
            replace_if_unchanged(&auth_path, &original_auth_bytes, previous_auth)
        }
        None => remove_if_unchanged(&auth_path, &original_auth_bytes),
    };
    if let Err(error) = auth_result {
        return Err(with_rollback(
            error,
            rollback_file(&config_path, &restored_config, &original_config_bytes),
        ));
    }
    if let Err(error) = remove_if_unchanged(&backup_path, &backup_bytes) {
        let auth_rollback =
            restore_snapshot_if_unchanged(&auth_path, &restored_auth_bytes, &original_auth_bytes);
        let config_rollback = rollback_file(&config_path, &restored_config, &original_config_bytes);
        return Err(with_rollback(
            error,
            merge_rollbacks(auth_rollback, config_rollback),
        ));
    }
    if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
        if let Err(error) = secrets.delete(secret_ref) {
            let backup_rollback = restore_snapshot_if_unchanged(&backup_path, &None, &backup_bytes);
            let auth_rollback = restore_snapshot_if_unchanged(
                &auth_path,
                &restored_auth_bytes,
                &original_auth_bytes,
            );
            let config_rollback =
                rollback_file(&config_path, &restored_config, &original_config_bytes);
            return Err(with_rollback(
                error,
                merge_rollbacks(
                    backup_rollback,
                    merge_rollbacks(auth_rollback, config_rollback),
                ),
            ));
        }
    }
    Ok(Some(binding_from_backup(&backup, true)))
}

fn sync_account_profile_with(
    codex_home: &Path,
    backup_root: &Path,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<bool> {
    let profile_dir = canonical_profile_dir(codex_home)?;
    let backup_path = account_backup_path(backup_root, &profile_dir);
    let backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_account_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(false);
    };
    let next_hash = key_hash(tokens.access_token());
    if backup.managed_access_hash == next_hash {
        return Ok(false);
    }
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let config = read_optional_bytes(&config_path)?;
    let auth = read_optional_bytes(&auth_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    let auth_matches_previous =
        account_auth_matches_snapshot(&auth, &auth_path, &backup.managed_access_hash)?;
    let auth_matches_next = account_auth_matches_snapshot(&auth, &auth_path, &next_hash)?;
    if !account_managed_config_matches(&document) || (!auth_matches_previous && !auth_matches_next)
    {
        return Ok(false);
    }
    backup.managed_access_hash = next_hash;
    let updated_backup = serialize_account_backup(&backup)?;
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
    .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
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
    let content = serde_json::to_string_pretty(backup)
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
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

fn parse_config(content: &str) -> Result<DocumentMut> {
    content.parse::<DocumentMut>().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("ChatGPT config is not valid TOML: {error}"),
        )
    })
}

fn validate_config_shape(document: &DocumentMut) -> Result<()> {
    if document.get("model_provider").is_some()
        && document
            .get("model_provider")
            .and_then(Item::as_str)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_provider must be a string",
        ));
    }
    if document.get("model_providers").is_some()
        && document
            .get("model_providers")
            .and_then(Item::as_table)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_providers must be a table",
        ));
    }
    if document.get("model_reasoning_effort").is_some()
        && document
            .get("model_reasoning_effort")
            .and_then(Item::as_str)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_reasoning_effort must be a string",
        ));
    }
    Ok(())
}

fn attach_config(
    document: &mut DocumentMut,
    base_url: &str,
    local_key: &str,
    model_catalog_path: Option<&str>,
    previous_model_catalog: Option<&str>,
) {
    // A global Codex effort overrides each model catalog entry. While Relay is
    // active it must be absent so automatic models use Relay's `medium`
    // default and a manual per-model rule remains authoritative.
    document.remove("model_reasoning_effort");
    document["model_provider"] = value(PROVIDER_ID);
    restore_root_string(
        document,
        "model_catalog_json",
        model_catalog_path.or(previous_model_catalog),
    );
    if document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_none()
    {
        document["model_providers"] = Item::Table(Table::new());
    }
    document["model_providers"][PROVIDER_ID] = Item::Table(Table::new());
    let provider = &mut document["model_providers"][PROVIDER_ID];
    provider["name"] = value("Zenith Relay Local");
    provider["base_url"] = value(base_url);
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(true);
    provider["experimental_bearer_token"] = value(local_key);
    provider["supports_websockets"] = value(false);
}

fn restore_config(
    document: &mut DocumentMut,
    previous_model_provider: Option<&str>,
    previous_model_catalog: Option<&str>,
) {
    remove_managed_provider(document);
    restore_root_string(document, "model_provider", previous_model_provider);
    restore_root_string(document, "model_catalog_json", previous_model_catalog);
}

fn restore_local_config(
    document: &mut DocumentMut,
    backup: &ProfileBackup,
    previous_model_catalog: Option<&str>,
) {
    restore_config(
        document,
        backup.previous_model_provider.as_deref(),
        previous_model_catalog,
    );
    if backup.managed_model_reasoning_effort_cleared {
        restore_root_string(
            document,
            "model_reasoning_effort",
            backup.previous_model_reasoning_effort.as_deref(),
        );
    }
}

const MANAGED_SNAPSHOT_AUTH_KEYS: &[&str] =
    &["OPENAI_API_KEY", "auth_mode", "last_refresh", "tokens"];

fn managed_snapshot_scope(
    profile_dir: &Path,
    backup_root: &Path,
    document: &DocumentMut,
    auth: &Option<Vec<u8>>,
    auth_path: &Path,
) -> Result<ManagedSnapshotScope> {
    if let Some(path) = account_backup_for_profile(profile_dir, backup_root)? {
        let bytes = read_optional_bytes(&path)?;
        let backup = parse_account_backup_snapshot(&bytes, &path)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT account profile backup disappeared during snapshot restore",
            )
        })?;
        if backup.profile_dir != profile_dir.to_string_lossy()
            || !account_managed_config_matches(document)
            || !account_auth_matches_snapshot(auth, auth_path, &backup.managed_access_hash)?
        {
            return Err(profile_restore_blocked());
        }
        return Ok(ManagedSnapshotScope::OAuthAccount);
    }

    let Some(backup) = local_backup(profile_dir, backup_root)? else {
        return Ok(ManagedSnapshotScope::NoBinding);
    };
    if !managed_config_matches(document, &backup)
        || !managed_auth_matches_snapshot(auth, auth_path, &backup)?
    {
        return Err(profile_restore_blocked());
    }
    Ok(ManagedSnapshotScope::LocalGateway)
}

fn merge_managed_snapshot_config(
    current: Option<&str>,
    snapshot: Option<&str>,
    scope: ManagedSnapshotScope,
) -> Result<Option<String>> {
    let mut document = parse_config(current.unwrap_or_default())?;
    validate_config_shape(&document)?;
    let snapshot = snapshot.map(parse_config).transpose()?;
    if let Some(snapshot) = snapshot.as_ref() {
        validate_config_shape(snapshot)?;
    }
    let before = document.to_string();

    let model_provider = snapshot.as_ref().and_then(root_model_provider);
    if model_provider.as_deref() == Some(PROVIDER_ID) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "a Relay-managed snapshot cannot restore another Relay binding",
        ));
    }
    restore_root_string(&mut document, "model_provider", model_provider.as_deref());
    let snapshot_value = |key| {
        snapshot
            .as_ref()
            .map(|snapshot| optional_root_string(snapshot, key))
            .transpose()
            .map(Option::flatten)
    };
    match scope {
        ManagedSnapshotScope::LocalGateway => {
            let catalog = snapshot_value("model_catalog_json")?;
            restore_root_string(&mut document, "model_catalog_json", catalog.as_deref());
        }
        ManagedSnapshotScope::OAuthAccount => {
            let base_url = snapshot_value("openai_base_url")?;
            restore_root_string(&mut document, "openai_base_url", base_url.as_deref());
        }
        ManagedSnapshotScope::NoBinding => {}
    }
    // Snapshot recovery always detaches Relay. Re-creating its provider here
    // would leave Codex pointed at a credential whose backup was discarded.
    remove_managed_provider(&mut document);

    let restored = document.to_string();
    if restored == before {
        return Ok(current.map(ToOwned::to_owned));
    }
    if restored.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(restored))
}

fn optional_root_string(document: &DocumentMut, key: &str) -> Result<Option<String>> {
    let Some(item) = document.get(key) else {
        return Ok(None);
    };
    item.as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("ChatGPT {key} must be a string"),
            )
        })
        .map(Some)
}

fn merge_managed_snapshot_auth(
    current: Option<&str>,
    snapshot: Option<&str>,
) -> Result<Option<String>> {
    let snapshot = snapshot
        .map(parse_profile_auth)
        .transpose()?
        .unwrap_or_default();
    let mut current_value = match current {
        Some(content) => parse_profile_auth(content)?,
        None => serde_json::Map::new(),
    };
    let before = current_value.clone();
    for key in MANAGED_SNAPSHOT_AUTH_KEYS {
        match snapshot.get(*key) {
            Some(value) => {
                current_value.insert((*key).to_string(), value.clone());
            }
            None => {
                current_value.remove(*key);
            }
        }
    }
    if current_value == before {
        return Ok(current.map(ToOwned::to_owned));
    }
    if current_value.is_empty() {
        return Ok(None);
    }
    let content = serde_json::to_string_pretty(&serde_json::Value::Object(current_value))
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    Ok(Some(format!("{content}\n")))
}

fn parse_profile_auth(content: &str) -> Result<serde_json::Map<String, serde_json::Value>> {
    let value = serde_json::from_str::<serde_json::Value>(content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("ChatGPT auth is not valid JSON: {error}"),
        )
    })?;
    value.as_object().cloned().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT auth must contain a JSON object",
        )
    })
}

fn restore_root_string(document: &mut DocumentMut, key: &str, previous: Option<&str>) {
    match previous {
        Some(previous) => document[key] = value(previous),
        None => {
            document.remove(key);
        }
    }
}

fn remove_managed_provider(document: &mut DocumentMut) {
    if let Some(model_providers) = document["model_providers"].as_table_mut() {
        model_providers.remove(PROVIDER_ID);
        if model_providers.is_empty() {
            document.remove("model_providers");
        }
    }
}

fn root_model_provider(document: &DocumentMut) -> Option<String> {
    document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

fn root_model_catalog_json(document: &DocumentMut) -> Option<String> {
    document
        .get("model_catalog_json")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

fn root_model_reasoning_effort(document: &DocumentMut) -> Option<String> {
    document
        .get("model_reasoning_effort")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

fn external_model_catalog(document: &DocumentMut, backup: &ProfileBackup) -> Option<String> {
    let current = root_model_catalog_json(document);
    if current.as_deref() == backup.managed_model_catalog_path.as_deref() {
        backup.previous_model_catalog_json.clone()
    } else {
        current
    }
}

fn model_catalog_to_restore(document: &DocumentMut, backup: &ProfileBackup) -> Option<String> {
    if backup.managed_model_catalog_path.is_some() {
        backup.previous_model_catalog_json.clone()
    } else {
        root_model_catalog_json(document)
    }
}

fn root_openai_base_url(document: &DocumentMut) -> Option<String> {
    document
        .get("openai_base_url")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

fn document_has_provider(document: &DocumentMut) -> bool {
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| providers.contains_key(PROVIDER_ID))
}

fn managed_config_matches(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    root_model_provider(document).as_deref() == Some(PROVIDER_ID)
        && (backup.managed_model_catalog_path.is_none()
            || root_model_catalog_json(document).as_deref()
                == backup.managed_model_catalog_path.as_deref())
        && managed_provider_matches(document, backup)
        && (!backup.managed_model_reasoning_effort_cleared
            || document.get("model_reasoning_effort").is_none())
}

fn previous_config_matches(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    root_model_provider(document) == backup.previous_model_provider
        && root_model_catalog_json(document) == backup.previous_model_catalog_json
        && (!backup.managed_model_reasoning_effort_cleared
            || root_model_reasoning_effort(document) == backup.previous_model_reasoning_effort)
}

fn external_provider_took_over(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    root_model_provider(document).is_some_and(|provider| provider != PROVIDER_ID)
        && managed_provider_matches(document, backup)
}

fn external_account_provider_took_over(codex_home: &Path) -> Result<bool> {
    let config_path = canonical_profile_dir(codex_home)?.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    Ok(root_model_provider(&document)
        .is_some_and(|provider| provider != "openai" && provider != PROVIDER_ID))
}

fn managed_provider_matches(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .and_then(Item::as_table)
        .is_some_and(|provider| {
            provider
                .get("name")
                .and_then(Item::as_str)
                .is_some_and(|name| name == "Zenith Relay Local")
                && provider
                    .get("base_url")
                    .and_then(Item::as_str)
                    .is_some_and(|base_url| {
                        base_url.trim_end_matches('/') == backup.managed_base_url
                    })
                && provider
                    .get("wire_api")
                    .and_then(Item::as_str)
                    .is_some_and(|wire_api| wire_api == "responses")
                && provider.get("requires_openai_auth").and_then(Item::as_bool) == Some(true)
                && (!backup.managed_bearer_in_config
                    || provider
                        .get("experimental_bearer_token")
                        .and_then(Item::as_str)
                        .is_some_and(|token| key_hash(token.trim()) == backup.managed_key_hash))
                && provider.get("supports_websockets").and_then(Item::as_bool)
                    == Some(backup.managed_supports_websockets)
        })
}

fn auth_content(local_key: &str) -> String {
    format!(
        "{{\n  \"OPENAI_API_KEY\": \"{}\",\n  \"auth_mode\": \"apikey\"\n}}\n",
        escape_json_string(local_key)
    )
}

fn auth_matches_snapshot(
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
    Ok(value
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| key_hash(key.trim()) == expected_hash)
        && value
            .get("auth_mode")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|mode| mode == "apikey"))
}

fn managed_auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
    backup: &ProfileBackup,
) -> Result<bool> {
    match (
        backup.bound_oauth_account_id.as_deref(),
        backup.managed_oauth_access_hash.as_deref(),
    ) {
        (Some(account_id), Some(access_hash))
            if !account_id.trim().is_empty() && access_hash.len() == 64 =>
        {
            account_auth_matches_snapshot(snapshot, path, access_hash)
        }
        (Some(account_id), None) if !account_id.trim().is_empty() => {
            auth_matches_snapshot(snapshot, path, &backup.managed_key_hash)
        }
        (None, None) => auth_matches_snapshot(snapshot, path, &backup.managed_key_hash),
        _ => Ok(false),
    }
}

fn previous_auth_matches_snapshot(snapshot: &Option<Vec<u8>>, backup: &ProfileBackup) -> bool {
    match backup.previous_auth_hash.as_deref() {
        Some(expected_hash) => snapshot
            .as_deref()
            .is_some_and(|content| bytes_hash(content) == expected_hash),
        None if backup.previous_auth_secret_ref.is_none() => snapshot.is_none(),
        None => false,
    }
}

fn key_hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn bytes_hash(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn reconcile_pending_catalog_state(
    backup_path: &Path,
    backup_bytes: &mut Option<Vec<u8>>,
    backup: &mut ProfileBackup,
    catalog_bytes: &Option<Vec<u8>>,
) -> Result<()> {
    let Some(pending_hash) = backup.managed_model_catalog_pending_hash.clone() else {
        return Ok(());
    };
    let current_hash = catalog_bytes.as_deref().map(bytes_hash);
    let stable_hash = backup.managed_model_catalog_hash.clone();
    if current_hash.as_deref() == Some(pending_hash.as_str()) {
        backup.managed_model_catalog_hash = Some(pending_hash);
        backup.managed_model_catalog_pending_hash = None;
    } else if current_hash == stable_hash || (stable_hash.is_none() && catalog_bytes.is_none()) {
        backup.managed_model_catalog_pending_hash = None;
    } else {
        return Err(profile_restore_blocked());
    }
    let updated = serialize_backup(backup)?;
    replace_if_unchanged(backup_path, backup_bytes, &updated)?;
    *backup_bytes = Some(updated.into_bytes());
    Ok(())
}

fn valid_managed_model_catalog(
    backup: &ProfileBackup,
    expected_path: &Path,
    content: &Option<Vec<u8>>,
) -> bool {
    if backup
        .managed_model_catalog_hash
        .as_deref()
        .is_some_and(|hash| hash.len() != 64)
        || backup
            .managed_model_catalog_pending_hash
            .as_deref()
            .is_some_and(|hash| hash.len() != 64)
        || (backup.managed_model_catalog_pending_remove
            && backup.managed_model_catalog_pending_hash.is_some())
    {
        return false;
    }
    let Some(path) = backup.managed_model_catalog_path.as_deref() else {
        return backup.managed_model_catalog_hash.is_none()
            && backup.managed_model_catalog_pending_hash.is_none()
            && !backup.managed_model_catalog_pending_remove
            && content.is_none();
    };
    if Path::new(path) != expected_path {
        return false;
    }
    if backup.restore_pending && content.is_none() {
        return true;
    }
    let current_hash = content.as_deref().map(bytes_hash);
    let stable_valid = match backup.managed_model_catalog_hash.as_deref() {
        Some(hash) if hash.len() == 64 => current_hash.as_deref() == Some(hash),
        None => content.is_none(),
        _ => false,
    };
    let pending_valid = backup
        .managed_model_catalog_pending_hash
        .as_deref()
        .is_some_and(|hash| hash.len() == 64 && current_hash.as_deref() == Some(hash));
    let pending_remove_valid = backup.managed_model_catalog_pending_remove && content.is_none();
    stable_valid || pending_valid || pending_remove_valid
}

fn managed_model_catalog_path(backup_root: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(backup_root).map_err(io_error)?;
    Ok(root.join(MODEL_CATALOG_FILE))
}

fn apply_model_catalog_change(
    path: &Path,
    previous: &Option<Vec<u8>>,
    next: Option<&str>,
    previously_managed: bool,
) -> Result<()> {
    match (next, previously_managed) {
        (Some(content), _) => replace_if_unchanged(path, previous, content),
        (None, true) => remove_if_unchanged(path, previous),
        (None, false) => Ok(()),
    }
}

fn rollback_model_catalog_change(
    path: &Path,
    attempted: Option<&str>,
    previously_managed: bool,
    previous: &Option<Vec<u8>>,
) -> Result<()> {
    match (attempted, previously_managed) {
        (Some(content), _) => rollback_file(path, content, previous),
        (None, true) => restore_snapshot_if_unchanged(path, &None, previous),
        (None, false) => Ok(()),
    }
}

fn remove_managed_model_catalog_if_unchanged(backup: &ProfileBackup) {
    let Some(path) = backup.managed_model_catalog_path.as_deref() else {
        return;
    };
    let path = Path::new(path);
    let Ok(Some(content)) = read_optional_bytes(path) else {
        return;
    };
    let current_hash = bytes_hash(&content);
    if backup.managed_model_catalog_hash.as_deref() == Some(current_hash.as_str())
        || backup.managed_model_catalog_pending_hash.as_deref() == Some(current_hash.as_str())
    {
        let _ = remove_if_unchanged(path, &Some(content));
    }
}

fn invalidate_models_cache(codex_home: &Path) -> Result<bool> {
    let path = codex_home.join(MODELS_CACHE_FILE);
    let snapshot = read_optional_bytes(&path)?;
    if snapshot.is_none() {
        return Ok(false);
    }
    remove_if_unchanged(&path, &snapshot)?;
    Ok(true)
}

fn backup_path(root: &Path) -> std::path::PathBuf {
    root.join("codex-default.json")
}

fn local_backup(codex_home: &Path, root: &Path) -> Result<Option<ProfileBackup>> {
    let path = backup_path(root);
    let mut snapshot = read_optional_bytes(&path)?;
    let Some(mut backup) = parse_backup_snapshot(&snapshot, &path)? else {
        return Ok(None);
    };
    let catalog_path = managed_model_catalog_path(root)?;
    let catalog = read_optional_bytes(&catalog_path)?;
    migrate_legacy_managed_catalog_metadata(
        codex_home,
        &path,
        &mut snapshot,
        &mut backup,
        &catalog_path,
        &catalog,
    )?;
    let oauth_metadata_valid = match (
        backup.bound_oauth_account_id.as_deref(),
        backup.managed_oauth_access_hash.as_deref(),
    ) {
        (Some(account_id), Some(access_hash)) => {
            !account_id.trim().is_empty() && access_hash.len() == 64
        }
        (Some(account_id), None) => !account_id.trim().is_empty(),
        (None, None) => true,
        _ => false,
    };
    if backup.version != 1
        || backup.managed_key_hash.len() != 64
        || backup.managed_base_url.trim().is_empty()
        || !oauth_metadata_valid
        || !valid_managed_model_catalog(&backup, &catalog_path, &catalog)
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT local gateway profile backup has invalid metadata",
        ));
    }
    Ok(Some(backup))
}

fn migrate_legacy_managed_catalog_metadata(
    codex_home: &Path,
    backup_path: &Path,
    backup_bytes: &mut Option<Vec<u8>>,
    backup: &mut ProfileBackup,
    catalog_path: &Path,
    catalog: &Option<Vec<u8>>,
) -> Result<()> {
    let legacy_metadata = backup.managed_model_catalog_path.is_none()
        && backup.managed_model_catalog_hash.is_none()
        && backup.managed_model_catalog_pending_hash.is_none()
        && !backup.managed_model_catalog_pending_remove;
    let Some(content) = catalog.as_deref() else {
        return Ok(());
    };
    if !legacy_metadata || !is_relay_managed_model_catalog(content) {
        return Ok(());
    }

    backup.managed_model_catalog_path = Some(catalog_path.to_string_lossy().into_owned());
    backup.managed_model_catalog_hash = Some(bytes_hash(content));
    if backup.previous_model_catalog_json.is_none() {
        let config_path = codex_home.join(CONFIG_FILE);
        let current_catalog = read_optional_bytes(&config_path)
            .ok()
            .flatten()
            .and_then(|config| {
                snapshot_text(&Some(config), &config_path)
                    .ok()
                    .flatten()
                    .map(str::to_owned)
            })
            .and_then(|content| parse_config(&content).ok())
            .and_then(|document| root_model_catalog_json(&document));
        if current_catalog
            .as_deref()
            .is_some_and(|path| !configured_catalog_matches_path(codex_home, path, catalog_path))
        {
            backup.previous_model_catalog_json = current_catalog;
        }
    }

    let updated = serialize_backup(backup)?;
    replace_if_unchanged(backup_path, backup_bytes, &updated)?;
    *backup_bytes = Some(updated.into_bytes());
    Ok(())
}

fn configured_catalog_matches_path(codex_home: &Path, configured: &str, expected: &Path) -> bool {
    let configured = Path::new(configured);
    let resolved = if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        codex_home.join(configured)
    };
    resolved == expected
}

fn is_relay_managed_model_catalog(content: &[u8]) -> bool {
    read_catalog_values(content, true).is_ok_and(|models| {
        !models.is_empty()
            && models.iter().all(|model| {
                model.get("comp_hash").and_then(Value::as_str) == Some(CODEX_RELAY_CATALOG_HASH)
            })
    })
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
    let content = serde_json::to_string_pretty(backup)
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
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

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error_at(path, error)),
    }
}

fn snapshot_text<'a>(snapshot: &'a Option<Vec<u8>>, path: &Path) -> Result<Option<&'a str>> {
    snapshot
        .as_deref()
        .map(|content| {
            std::str::from_utf8(content).map_err(|error| {
                LocalPoolError::new(
                    ErrorCode::Io,
                    format!("{} is not valid UTF-8: {error}", path.display()),
                )
            })
        })
        .transpose()
}

fn replace_if_unchanged(path: &Path, expected: &Option<Vec<u8>>, content: &str) -> Result<()> {
    if &read_optional_bytes(path)? != expected {
        return Err(profile_changed_at(path));
    }
    atomic_write(path, content).map_err(io_error_message)
}

fn remove_if_unchanged(path: &Path, expected: &Option<Vec<u8>>) -> Result<()> {
    if &read_optional_bytes(path)? != expected {
        return Err(profile_changed_at(path));
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && expected.is_none() => Ok(()),
        Err(error) => Err(io_error_at(path, error)),
    }
}

fn rollback_file(path: &Path, expected_content: &str, previous: &Option<Vec<u8>>) -> Result<()> {
    let expected = Some(expected_content.as_bytes().to_vec());
    restore_snapshot_if_unchanged(path, &expected, previous)
}

fn restore_snapshot_if_unchanged(
    path: &Path,
    expected_current: &Option<Vec<u8>>,
    previous: &Option<Vec<u8>>,
) -> Result<()> {
    match snapshot_text(previous, path)? {
        Some(content) => replace_if_unchanged(path, expected_current, content),
        None => remove_if_unchanged(path, expected_current),
    }
}

fn replace_with_snapshot(
    path: &Path,
    expected_current: &Option<Vec<u8>>,
    content: Option<&str>,
) -> Result<()> {
    match content {
        Some(content) => replace_if_unchanged(path, expected_current, content),
        None => remove_if_unchanged(path, expected_current),
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

fn merge_rollbacks(first: Result<()>, second: Result<()>) -> Result<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{}; {}", first.message, second.message),
        )),
    }
}

fn with_rollback(error: LocalPoolError, rollback: Result<()>) -> LocalPoolError {
    match rollback {
        Ok(()) => error,
        Err(rollback_error) => LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; profile rollback failed: {}",
                error.message, rollback_error.message
            ),
        ),
    }
}

fn profile_restore_blocked() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        "ChatGPT profile changed after attach; restore was not applied",
    )
}

fn profile_changed_at(path: &Path) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        format!(
            "ChatGPT changed {} while Zenith Relay was updating the profile; no replacement was applied",
            path.display()
        ),
    )
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

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error.to_string())
}

fn io_error_at(path: &Path, error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("failed to access {}: {error}", path.display()),
    )
}

fn io_error_message(error: String) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, path::PathBuf, sync::Mutex};

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl SecretBackend for MemorySecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(secret_ref.into(), value.into());
            Ok(())
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(secret_ref).cloned())
        }

        fn delete(&self, secret_ref: &str) -> Result<()> {
            self.0.lock().unwrap().remove(secret_ref);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingDeleteSecrets(MemorySecrets);

    impl SecretBackend for FailingDeleteSecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
            self.0.save(secret_ref, value)
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>> {
            self.0.load(secret_ref)
        }

        fn delete(&self, _secret_ref: &str) -> Result<()> {
            Err(LocalPoolError::new(
                ErrorCode::SecretStoreUnavailable,
                "injected delete failure",
            ))
        }
    }

    struct MutatingSecrets {
        values: Mutex<HashMap<String, String>>,
        path: PathBuf,
        content: Vec<u8>,
    }

    impl MutatingSecrets {
        fn new(path: PathBuf, content: impl Into<Vec<u8>>) -> Self {
            Self {
                values: Mutex::new(HashMap::new()),
                path,
                content: content.into(),
            }
        }
    }

    impl SecretBackend for MutatingSecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
            self.values
                .lock()
                .unwrap()
                .insert(secret_ref.into(), value.into());
            fs::write(&self.path, &self.content).map_err(io_error)
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>> {
            Ok(self.values.lock().unwrap().get(secret_ref).cloned())
        }

        fn delete(&self, secret_ref: &str) -> Result<()> {
            self.values.lock().unwrap().remove(secret_ref);
            Ok(())
        }
    }

    struct MutatingLoadSecrets {
        values: Mutex<HashMap<String, String>>,
        path: PathBuf,
        content: Vec<u8>,
    }

    impl MutatingLoadSecrets {
        fn new(path: PathBuf, content: impl Into<Vec<u8>>) -> Self {
            Self {
                values: Mutex::new(HashMap::new()),
                path,
                content: content.into(),
            }
        }
    }

    impl SecretBackend for MutatingLoadSecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
            self.values
                .lock()
                .unwrap()
                .insert(secret_ref.into(), value.into());
            Ok(())
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>> {
            fs::write(&self.path, &self.content).map_err(io_error)?;
            Ok(self.values.lock().unwrap().get(secret_ref).cloned())
        }

        fn delete(&self, secret_ref: &str) -> Result<()> {
            self.values.lock().unwrap().remove(secret_ref);
            Ok(())
        }
    }

    #[test]
    fn missing_backup_directory_has_no_local_binding() {
        let (root, home, backups) = profile_dirs("missing-backup-root");
        assert!(local_backup(&home, &backups).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attach_and_restore_preserve_previous_profile_and_nested_provider() {
        let (root, home, backups) = profile_dirs("restore");
        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"openai\"\n\n[profiles.default]\nmodel_provider = \"custom\"\n",
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"secret\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        restore_with(&home, &backups, &secrets).unwrap();

        let config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
        assert!(config.contains("model_provider = \"openai\""));
        assert!(config.contains("model_provider = \"custom\""));
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("chatgpt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_gateway_uses_catalog_reasoning_default_and_restores_global_override() {
        let (root, home, backups) = profile_dirs("reasoning-effort-override");
        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"openai\"\nmodel_reasoning_effort = \"ultra\"\n",
        )
        .unwrap();
        let secrets = MemorySecrets::default();

        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();

        let managed_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
        assert!(!managed_config.contains("model_reasoning_effort"));
        let backup = local_backup(&home, &backups)
            .unwrap()
            .expect("profile backup");
        assert_eq!(
            backup.previous_model_reasoning_effort.as_deref(),
            Some("ultra")
        );
        assert!(backup.managed_model_reasoning_effort_cleared);

        restore_with(&home, &backups, &secrets).unwrap();
        let restored_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
        assert!(restored_config.contains("model_provider = \"openai\""));
        assert!(restored_config.contains("model_reasoning_effort = \"ultra\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_catalog_attach_and_restore_preserve_user_config_and_cache() {
        let (root, home, backups) = profile_dirs("model-catalog-restore");
        let previous_catalog_path = root.join("previous-codex-models.json");
        write_test_catalog_file(&previous_catalog_path, "native-user-model");
        let previous_catalog = previous_catalog_path.to_string_lossy().replace('\\', "/");
        fs::write(
            home.join(CONFIG_FILE),
            format!("model_provider = \"openai\"\nmodel_catalog_json = \"{previous_catalog}\"\n"),
        )
        .unwrap();
        let cache_path = home.join(MODELS_CACHE_FILE);
        let fresh_cache =
            r#"{"fetched_at":"2026-07-30T00:00:00Z","etag":"v1","models":[{"slug":"cached"}]}"#;
        fs::write(&cache_path, fresh_cache).unwrap();
        let secrets = MemorySecrets::default();
        let catalog = r#"{"models":[{"slug":"vendor/claude-opus-4-8","service_tiers":[{"id":"priority","name":"Fast","description":"Fast tier"}],"additional_speed_tiers":["fast"],"default_service_tier":"priority","default_reasoning_level":"high","supported_reasoning_levels":[{"effort":"low","description":"Low"},{"effort":"high","description":"High"},{"effort":"ultra","description":"Ultra"}],"supports_reasoning_summary_parameter":true,"supports_reasoning_summaries":true,"default_reasoning_summary":"detailed","supports_parallel_tool_calls":true}]}"#;

        attach_with_catalog_for_test(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            catalog,
            &secrets,
        )
        .unwrap();

        let catalog_path = managed_model_catalog_path(&backups).unwrap();
        let attached = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
        assert_eq!(
            root_model_catalog_json(&attached).as_deref(),
            Some(catalog_path.to_string_lossy().as_ref())
        );
        let managed_catalog: Value =
            serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
        let models = managed_catalog["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["slug"], "vendor/claude-opus-4-8");
        assert_ne!(models[0]["slug"], "native-user-model");
        assert_eq!(models[0]["default_reasoning_level"], "high");
        assert_eq!(
            models[0]["supported_reasoning_levels"][2]["effort"],
            "ultra"
        );
        assert_eq!(models[0]["service_tiers"][0]["id"], "priority");
        assert_eq!(models[0]["additional_speed_tiers"], json!(["fast"]));
        assert_eq!(models[0]["default_service_tier"], "priority");
        assert_eq!(models[0]["supports_reasoning_summary_parameter"], true);
        assert_eq!(models[0]["supports_reasoning_summaries"], true);
        assert_eq!(models[0]["default_reasoning_summary"], "detailed");
        assert_eq!(models[0]["supports_parallel_tool_calls"], true);
        assert!(!cache_path.exists());

        fs::write(&cache_path, fresh_cache).unwrap();
        restore_with(&home, &backups, &secrets).unwrap();

        let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
        assert_eq!(
            root_model_catalog_json(&restored).as_deref(),
            Some(previous_catalog.as_str())
        );
        assert!(!catalog_path.exists());
        assert!(!cache_path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_source_catalog_contains_only_selected_source_models_without_native_capabilities() {
        let (root, home, _backups) = profile_dirs("direct-source-catalog");
        let mut native = routed_codex_catalog_entry(None, "gpt-5.6-sol", 1, None);
        native["slug"] = Value::String("gpt-5.6-sol".into());
        native["display_name"] = Value::String("GPT-5.6 Sol".into());
        native["description"] = Value::String("Native test model".into());
        native["comp_hash"] = Value::String("official".into());
        native["default_reasoning_level"] = Value::String("low".into());
        native["supported_reasoning_levels"] = json!([
            {"effort": "low", "description": "Low"},
            {"effort": "ultra", "description": "Ultra"}
        ]);
        let mut relay_owned = routed_codex_catalog_entry(None, "gpt-fake", 2, None);
        relay_owned["slug"] = Value::String("gpt-fake".into());
        relay_owned["comp_hash"] = Value::String(CODEX_RELAY_CATALOG_HASH.into());
        fs::write(
            home.join(MODELS_CACHE_FILE),
            serde_json::to_string_pretty(&json!({"models": [native, relay_owned]})).unwrap(),
        )
        .unwrap();

        let catalog = direct_source_model_catalog(
            &home,
            &[
                "gpt-5.6-sol".into(),
                "vendor/claude".into(),
                "gpt-fake".into(),
                "zenith/alias".into(),
            ],
        )
        .unwrap()
        .expect("catalog");
        let models = serde_json::from_str::<Value>(&catalog).unwrap()["models"]
            .as_array()
            .unwrap()
            .clone();

        assert_eq!(models.len(), 3);
        assert_eq!(models[0]["slug"], "gpt-5.6-sol");
        assert_eq!(models[1]["slug"], "gpt-fake");
        assert_eq!(models[2]["slug"], "vendor/claude");
        assert_eq!(
            models
                .iter()
                .map(|model| model["priority"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [1_000, 1_001, 1_002]
        );
        for model in &models {
            assert!(model.get("default_reasoning_level").is_none());
            assert_eq!(model["supported_reasoning_levels"], json!([]));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_source_catalog_allows_images_for_every_routed_model() {
        let (root, home, _backups) = profile_dirs("direct-source-image-capability");
        let manifest = json!({
            "data": [
                {
                    "id": "provider/vision",
                    "input_modalities": ["text", "image"]
                },
                {
                    "id": "provider/text",
                    "input_modalities": ["text"]
                }
            ]
        });

        let catalog = direct_source_model_catalog_with_manifest(
            &home,
            &["provider/vision".into(), "provider/text".into()],
            Some(&manifest),
        )
        .unwrap()
        .expect("catalog");
        let parsed_catalog = serde_json::from_str::<Value>(&catalog).unwrap();
        let models = parsed_catalog["models"].as_array().unwrap();

        assert_eq!(models[0]["slug"], "provider/vision");
        assert_eq!(models[0]["input_modalities"], json!(["text", "image"]));
        assert_eq!(models[1]["slug"], "provider/text");
        assert_eq!(models[1]["input_modalities"], json!(["text", "image"]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_source_catalog_uses_medium_for_automatic_reasoning() {
        let (root, home, _backups) = profile_dirs("direct-source-reasoning-default");
        let manifest = json!({
            "models": [{
                "slug": "provider/reasoning",
                "default_reasoning_level": "ultra",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Low"},
                    {"effort": "medium", "description": "Medium"},
                    {"effort": "ultra", "description": "Ultra"}
                ]
            }]
        });

        let catalog = direct_source_model_catalog_with_manifest(
            &home,
            &["provider/reasoning".into()],
            Some(&manifest),
        )
        .unwrap()
        .expect("direct catalog");
        let model = &serde_json::from_str::<Value>(&catalog).unwrap()["models"][0];

        assert_eq!(model["default_reasoning_level"], "medium");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_direct_source_catalog_does_not_restore_provider_ultra_default() {
        let (root, home, _backups) = profile_dirs("managed-direct-source-reasoning-default");
        let manifest = json!({
            "models": [{
                "slug": "provider/reasoning",
                "default_reasoning_level": "ultra",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Low"},
                    {"effort": "high", "description": "High"},
                    {"effort": "ultra", "description": "Ultra"}
                ]
            }]
        });
        let direct = direct_source_model_catalog_with_manifest(
            &home,
            &["provider/reasoning".into()],
            Some(&manifest),
        )
        .unwrap()
        .expect("direct catalog");

        let managed = build_managed_model_catalog(&home, None, None, &direct).unwrap();
        let model = &serde_json::from_str::<Value>(&managed).unwrap()["models"][0];

        assert!(model.get("default_reasoning_level").is_none());
        assert_eq!(
            model["supported_reasoning_levels"],
            json!([
                {"effort": "low", "description": "Low"},
                {"effort": "high", "description": "High"},
                {"effort": "ultra", "description": "Ultra"}
            ])
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_source_catalog_resolves_the_configured_relative_template() {
        let (root, home, _backups) = profile_dirs("direct-source-relative-template");
        write_test_catalog_file(&home.join("native-catalog.json"), "gpt-5.6-sol");
        fs::write(
            home.join(CONFIG_FILE),
            "model_catalog_json = \"native-catalog.json\"\n",
        )
        .unwrap();

        let catalog = direct_source_model_catalog(&home, &["vendor/claude-opus".into()])
            .unwrap()
            .expect("catalog");
        let models = serde_json::from_str::<Value>(&catalog).unwrap()["models"]
            .as_array()
            .unwrap()
            .clone();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["slug"], "vendor/claude-opus");
        assert_eq!(models[0]["supported_reasoning_levels"], json!([]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_catalog_preserves_native_model_settings() {
        let (root, home, _backups) = profile_dirs("managed-native-settings");
        let mut native = routed_codex_catalog_entry(None, "gpt-native", 1, None);
        native["slug"] = Value::String("gpt-native".into());
        native["comp_hash"] = Value::String("official".into());
        native["input_modalities"] = json!(["text", "image"]);
        native["default_reasoning_level"] = Value::String("ultra".into());
        native["supported_reasoning_levels"] = json!([
            {"effort": "low", "description": "Low"},
            {"effort": "ultra", "description": "Ultra"}
        ]);
        native["service_tiers"] = json!([{
            "id": "priority",
            "name": "Fast",
            "description": "Native fast tier"
        }]);
        native["default_service_tier"] = Value::String("priority".into());
        native["context_window"] = 128_000.into();
        native["max_context_window"] = 120_000.into();
        native["auto_compact_token_limit"] = 110_000.into();
        native["native_setting"] = Value::String("keep-me".into());
        let catalog = serde_json::to_string(&json!({"models": [native]})).unwrap();

        let managed = build_managed_model_catalog(&home, None, None, &catalog).unwrap();
        let model = &serde_json::from_str::<Value>(&managed).unwrap()["models"][0];

        assert_eq!(model["input_modalities"], json!(["text", "image"]));
        assert_eq!(model["default_reasoning_level"], "ultra");
        assert_eq!(
            model["supported_reasoning_levels"],
            json!([
                {"effort": "low", "description": "Low"},
                {"effort": "ultra", "description": "Ultra"}
            ])
        );
        assert_eq!(model["service_tiers"][0]["id"], "priority");
        assert_eq!(model["default_service_tier"], "priority");
        assert_eq!(model["context_window"], 128_000);
        assert_eq!(model["max_context_window"], 120_000);
        assert_eq!(model["auto_compact_token_limit"], 110_000);
        assert_eq!(model["native_setting"], "keep-me");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_catalogs_do_not_require_cached_native_metadata() {
        let (root, home, _backups) = profile_dirs("catalog-metadata-fallback");

        let direct = direct_source_model_catalog(&home, &["vendor/direct".into()])
            .unwrap()
            .expect("direct catalog");
        assert_eq!(
            serde_json::from_str::<Value>(&direct).unwrap()["models"][0]["slug"],
            "vendor/direct"
        );

        let managed = build_managed_model_catalog(
            &home,
            None,
            None,
            r#"{"models":[{"slug":"vendor/managed"}]}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&managed).unwrap()["models"][0]["slug"],
            "vendor/managed"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn active_managed_catalog_refreshes_without_replacing_the_profile() {
        let (root, home, backups) = profile_dirs("model-catalog-refresh");
        let cache_path = home.join(MODELS_CACHE_FILE);
        fs::write(
            &cache_path,
            r#"{"fetched_at":"2026-07-30T00:00:00Z","etag":"v1","models":[]}"#,
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with_catalog_for_test(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            r#"{"models":[{"slug":"old-model"}]}"#,
            &secrets,
        )
        .unwrap();

        assert!(refresh_managed_model_catalog(
            &home,
            &backups,
            r#"{"models":[{"slug":"new-model"}]}"#
        )
        .unwrap());
        let catalog_path = managed_model_catalog_path(&backups).unwrap();
        let catalog: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(catalog_path).unwrap()).unwrap();
        assert!(catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["slug"] == "new-model"));
        assert!(!cache_path.exists());
        assert!(!refresh_managed_model_catalog(
            &home,
            &backups,
            r#"{"models":[{"slug":"new-model"}]}"#
        )
        .unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_catalog_upgrade_preserves_legacy_user_catalog() {
        let (root, home, backups) = profile_dirs("legacy-model-catalog");
        let previous_catalog_path = root.join("legacy-models.json");
        write_test_catalog_file(&previous_catalog_path, "legacy-native-model");
        let previous_catalog = previous_catalog_path.to_string_lossy().replace('\\', "/");
        fs::write(
            home.join(CONFIG_FILE),
            format!("model_catalog_json = \"{previous_catalog}\"\n"),
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_old_key",
            &secrets,
        )
        .unwrap();
        let backup_path = backup_path(&backups);
        let mut legacy: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.remove("previousModelCatalogJson");
        object.remove("managedModelCatalogPath");
        object.remove("managedModelCatalogHash");
        fs::write(&backup_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        attach_with_catalog_for_test(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_new_key",
            r#"{"models":[{"slug":"vendor/model"}]}"#,
            &secrets,
        )
        .unwrap();
        restore_with(&home, &backups, &secrets).unwrap();

        let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
        assert_eq!(
            root_model_catalog_json(&restored).as_deref(),
            Some(previous_catalog.as_str())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_relay_catalog_metadata_is_adopted_without_overwriting_an_external_catalog() {
        let (root, home, backups) = profile_dirs("legacy-managed-catalog-metadata");
        let external_config =
            "model_provider = \"custom\"\nmodel_catalog_json = \"custom-catalog.json\"\n";
        write_test_catalog_file(&home.join("custom-catalog.json"), "native-user-model");
        fs::write(home.join(CONFIG_FILE), external_config).unwrap();
        let secrets = MemorySecrets::default();
        attach_with_catalog_for_test(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            r#"{"models":[{"slug":"vendor/model"}]}"#,
            &secrets,
        )
        .unwrap();

        let backup_path = backup_path(&backups);
        let mut legacy: Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        let object = legacy.as_object_mut().unwrap();
        for field in [
            "previousModelCatalogJson",
            "managedModelCatalogPath",
            "managedModelCatalogHash",
            "managedModelCatalogPendingHash",
            "managedModelCatalogPendingRemove",
        ] {
            object.remove(field);
        }
        fs::write(&backup_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();
        fs::write(home.join(CONFIG_FILE), external_config).unwrap();

        let backup = local_backup(&home, &backups).unwrap().expect("backup");
        let catalog_path = managed_model_catalog_path(&backups).unwrap();
        assert_eq!(
            backup.managed_model_catalog_path.as_deref(),
            Some(catalog_path.to_string_lossy().as_ref())
        );
        assert_eq!(
            backup.managed_model_catalog_hash.as_deref(),
            Some(bytes_hash(&fs::read(&catalog_path).unwrap()).as_str())
        );
        assert_eq!(
            backup.previous_model_catalog_json.as_deref(),
            Some("custom-catalog.json")
        );
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            external_config
        );

        restore_with(&home, &backups, &secrets).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            external_config
        );
        assert!(!catalog_path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_catalog_without_the_relay_marker_is_not_adopted() {
        let (root, home, backups) = profile_dirs("legacy-unowned-catalog-metadata");
        let secrets = MemorySecrets::default();
        attach_with_catalog_for_test(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            r#"{"models":[{"slug":"vendor/model"}]}"#,
            &secrets,
        )
        .unwrap();

        let backup_path = backup_path(&backups);
        let mut legacy: Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        let object = legacy.as_object_mut().unwrap();
        for field in [
            "managedModelCatalogPath",
            "managedModelCatalogHash",
            "managedModelCatalogPendingHash",
            "managedModelCatalogPendingRemove",
        ] {
            object.remove(field);
        }
        fs::write(&backup_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        let catalog_path = managed_model_catalog_path(&backups).unwrap();
        let mut catalog: Value =
            serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
        for model in catalog["models"].as_array_mut().unwrap() {
            model["comp_hash"] = Value::String("external-catalog".into());
        }
        fs::write(
            &catalog_path,
            serde_json::to_string_pretty(&catalog).unwrap(),
        )
        .unwrap();
        let original_backup = fs::read(&backup_path).unwrap();

        let error = local_backup(&home, &backups).unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert_eq!(fs::read(&backup_path).unwrap(), original_backup);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_discard_removes_only_an_unchanged_managed_catalog() {
        let catalog = r#"{"models":[{"slug":"vendor/model"}]}"#;
        for changed in [false, true] {
            let (root, home, backups) = profile_dirs(if changed {
                "discard-changed-catalog"
            } else {
                "discard-managed-catalog"
            });
            let secrets = MemorySecrets::default();
            attach_with_catalog_for_test(
                &home,
                &backups,
                "http://127.0.0.1:14998/v1",
                "zlr_key",
                catalog,
                &secrets,
            )
            .unwrap();
            let catalog_path = managed_model_catalog_path(&backups).unwrap();
            if changed {
                fs::write(&catalog_path, "externally changed").unwrap();
            }

            discard_managed_binding_locked(&home, &backups, &secrets).unwrap();

            assert_eq!(catalog_path.exists(), changed);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn user_snapshot_excludes_managed_projection_and_restore_detaches_it() {
        let (root, home, backups) = profile_dirs("user-snapshot");
        let original_config = "model_provider = \"custom\"\n";
        let original_auth = "{\"tokens\":{\"access_token\":\"original\"}}";
        fs::write(home.join(CONFIG_FILE), original_config).unwrap();
        fs::write(home.join(AUTH_FILE), original_auth).unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();

        let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
        assert_eq!(snapshot.config.as_deref(), Some(original_config));
        assert_eq!(snapshot.auth.as_deref(), Some(original_auth));
        assert!(!snapshot.config.as_deref().unwrap().contains(PROVIDER_ID));

        restore_user_profile_snapshot_full_with(&home, &backups, &snapshot, &secrets).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            original_config
        );
        assert_eq!(
            fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
            original_auth
        );
        assert_eq!(profile_backup_count(&backups), 0);
        assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_snapshot_restore_preserves_unmanaged_profile_data() {
        let (root, home, backups) = profile_dirs("managed-snapshot-merge");
        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"custom\"\nmodel_catalog_json = \"before.json\"\nopenai_base_url = \"https://old.example/v1\"\n[mcp_servers.context7]\ncommand = \"old\"\n[plugins.example]\nenabled = true\n[features]\nexperimental = false\n[model_providers.external]\nbase_url = \"https://external.example/v1\"\n",
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"OPENAI_API_KEY\":\"original-key\",\"last_refresh\":\"old\",\"tokens\":{\"access_token\":\"original\"},\"custom\":{\"keep\":true}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();

        let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
        let mut current =
            parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
        current["mcp_servers"]["context7"]["command"] = value("new");
        current["plugins"]["example"]["enabled"] = value(false);
        current["features"]["experimental"] = value(true);
        current["openai_base_url"] = value("https://changed-openai.example/v1");
        current["model_providers"]["external"]["base_url"] = value("https://changed.example/v1");
        fs::write(home.join(CONFIG_FILE), current.to_string()).unwrap();
        let mut current_auth: Value =
            serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
        current_auth["custom"] = json!({"keep": "current"});
        fs::write(
            home.join(AUTH_FILE),
            format!("{}\n", serde_json::to_string_pretty(&current_auth).unwrap()),
        )
        .unwrap();

        restore_user_profile_snapshot_managed_with(&home, &backups, &snapshot, &secrets).unwrap();

        let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
        assert_eq!(root_model_provider(&restored).as_deref(), Some("custom"));
        assert_eq!(
            root_model_catalog_json(&restored).as_deref(),
            Some("before.json")
        );
        assert_eq!(
            restored["openai_base_url"].as_str(),
            Some("https://changed-openai.example/v1")
        );
        assert_eq!(
            restored["mcp_servers"]["context7"]["command"].as_str(),
            Some("new")
        );
        assert_eq!(
            restored["plugins"]["example"]["enabled"].as_bool(),
            Some(false)
        );
        assert_eq!(restored["features"]["experimental"].as_bool(), Some(true));
        assert_eq!(
            restored["model_providers"]["external"]["base_url"].as_str(),
            Some("https://changed.example/v1")
        );
        assert!(restored
            .get("model_providers")
            .and_then(Item::as_table)
            .is_none_or(|providers| !providers.contains_key(PROVIDER_ID)));

        let restored_auth: Value =
            serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
        assert_eq!(restored_auth["OPENAI_API_KEY"], "original-key");
        assert_eq!(restored_auth["last_refresh"], "old");
        assert_eq!(restored_auth["tokens"]["access_token"], "original");
        assert_eq!(restored_auth["custom"]["keep"], "current");
        assert_eq!(profile_backup_count(&backups), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_snapshot_restore_accepts_a_detached_profile() {
        let (root, home, backups) = profile_dirs("managed-snapshot-detached");
        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"custom\"\nmodel_catalog_json = \"before.json\"\n[mcp_servers.context7]\ncommand = \"original\"\n",
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"apikey\",\"OPENAI_API_KEY\":\"original-key\",\"custom\":{\"keep\":\"original\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
        restore_with(&home, &backups, &secrets).unwrap();
        assert_eq!(profile_backup_count(&backups), 0);

        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"changed\"\nmodel_catalog_json = \"current.json\"\n[mcp_servers.context7]\ncommand = \"current\"\n",
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"apikey\",\"OPENAI_API_KEY\":\"current-key\",\"custom\":{\"keep\":\"current\"}}",
        )
        .unwrap();

        restore_user_profile_snapshot_managed_with(&home, &backups, &snapshot, &secrets).unwrap();

        let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
        assert_eq!(root_model_provider(&restored).as_deref(), Some("custom"));
        assert_eq!(
            root_model_catalog_json(&restored).as_deref(),
            Some("current.json")
        );
        assert_eq!(
            restored["mcp_servers"]["context7"]["command"].as_str(),
            Some("current")
        );
        let restored_auth: Value =
            serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
        assert_eq!(restored_auth["OPENAI_API_KEY"], "original-key");
        assert_eq!(restored_auth["custom"]["keep"], "current");
        assert_eq!(profile_backup_count(&backups), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_snapshot_restore_blocks_a_fresh_login() {
        let (root, home, backups) = profile_dirs("managed-snapshot-fresh-login");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"tokens\":{\"access_token\":\"original\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
        )
        .unwrap();
        let config_before = fs::read(home.join(CONFIG_FILE)).unwrap();
        let auth_before = fs::read(home.join(AUTH_FILE)).unwrap();

        let error =
            restore_user_profile_snapshot_managed_with(&home, &backups, &snapshot, &secrets)
                .unwrap_err();

        assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config_before);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth_before);
        assert!(backup_path(&backups).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_blocks_fresh_login_without_touching_files() {
        let (root, home, backups) = profile_dirs("fresh-login");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"chatgpt\"}").unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
        )
        .unwrap();
        let config_before = fs::read(home.join(CONFIG_FILE)).unwrap();
        let auth_before = fs::read(home.join(AUTH_FILE)).unwrap();

        let error = restore_with(&home, &backups, &secrets).unwrap_err();
        assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config_before);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth_before);
        assert!(backup_path(&backups).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_bindings_fail_closed_when_managed_provider_has_no_backup() {
        let (root, home, backups) = profile_dirs("missing-reset-backup");
        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"zenith_relay_local\"\n\n[model_providers.zenith_relay_local]\nname = \"Zenith Relay\"\n",
        )
        .unwrap();

        let error = profile_bindings(&home, &backups).unwrap_err();
        assert!(matches!(error.code, ErrorCode::RecoveryRequired));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_blocks_changed_provider_origin() {
        let (root, home, backups) = profile_dirs("changed-origin");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        let changed = fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .replace("14998", "14999");
        fs::write(home.join(CONFIG_FILE), changed).unwrap();
        assert!(matches!(
            restore_with(&home, &backups, &secrets).unwrap_err().code,
            ErrorCode::ProfileRestoreBlocked
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_blocks_changed_gateway_bearer() {
        let (root, home, backups) = profile_dirs("changed-bearer");
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        let changed = fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .replace("zlr_key", "zlr_other");
        fs::write(home.join(CONFIG_FILE), changed).unwrap();

        assert!(matches!(
            restore_with(&home, &backups, &secrets).unwrap_err().code,
            ErrorCode::ProfileRestoreBlocked
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_attach_upgrades_a_profile_without_managed_bearer_metadata() {
        let (root, home, backups) = profile_dirs("legacy-missing-bearer");
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        let config = fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .lines()
            .filter(|line| !line.trim_start().starts_with("experimental_bearer_token ="))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(home.join(CONFIG_FILE), config).unwrap();
        let backup_path = backup_path(&backups);
        let mut backup: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        backup
            .as_object_mut()
            .unwrap()
            .remove("managedBearerInConfig");
        fs::write(&backup_path, serde_json::to_string_pretty(&backup).unwrap()).unwrap();

        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .contains("experimental_bearer_token = \"zlr_key\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_attach_blocks_after_fresh_login() {
        let (root, home, backups) = profile_dirs("repeat-fresh-login");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
        )
        .unwrap();

        assert!(matches!(
            attach_with(
                &home,
                &backups,
                "http://127.0.0.1:14998/v1",
                "zlr_new_key",
                &secrets
            )
            .unwrap_err()
            .code,
            ErrorCode::ProfileRestoreBlocked
        ));
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("fresh"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_attach_rebases_external_takeover_and_restores_latest_profile() {
        let (root, home, backups) = profile_dirs("repeat-external-takeover");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();

        let legacy_config = fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .replace("supports_websockets = true", "supports_websockets = false");
        let backup_path = backup_path(&backups);
        let mut legacy_backup: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        legacy_backup
            .as_object_mut()
            .unwrap()
            .remove("managedSupportsWebsockets");
        fs::write(
            &backup_path,
            serde_json::to_string_pretty(&legacy_backup).unwrap(),
        )
        .unwrap();

        let external_config = legacy_config
            .replacen(
                "model_provider = \"zenith_relay_local\"",
                "model_provider = \"codex_local_access\"",
                1,
            )
            + "\n[model_providers.codex_local_access]\nname = \"Codex API Service\"\nbase_url = \"http://127.0.0.1:49976/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n";
        fs::write(home.join(CONFIG_FILE), external_config).unwrap();
        let external_auth = "{\"OPENAI_API_KEY\":null,\"tokens\":{\"access_token\":\"fresh\"}}";
        fs::write(home.join(AUTH_FILE), external_auth).unwrap();

        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_next_key",
            &secrets,
        )
        .unwrap();
        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .starts_with("model_provider = \"zenith_relay_local\""));
        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .contains("supports_websockets = false"));
        let upgraded_backup: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        assert_eq!(upgraded_backup["managedSupportsWebsockets"], false);

        restore_with(&home, &backups, &secrets).unwrap();
        let restored_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
        assert!(restored_config.starts_with("model_provider = \"codex_local_access\""));
        assert!(!restored_config.contains("[model_providers.zenith_relay_local]"));
        assert_eq!(
            fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
            external_auth
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_keeps_root_model_provider_absent_when_it_started_absent() {
        let (root, home, backups) = profile_dirs("no-root-provider");
        fs::write(
            home.join(CONFIG_FILE),
            "[profiles.default]\nmodel_provider = \"custom\"\n",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        restore_with(&home, &backups, &secrets).unwrap();

        let document = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
        assert!(document.get("model_provider").is_none());
        assert_eq!(
            document["profiles"]["default"]["model_provider"].as_str(),
            Some("custom")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attach_rejects_non_utf8_config_without_rewriting_it() {
        let (root, home, backups) = profile_dirs("non-utf8");
        let config_path = home.join(CONFIG_FILE);
        let original = vec![0xff, 0xfe, 0xfd];
        fs::write(&config_path, &original).unwrap();
        let secrets = MemorySecrets::default();

        assert!(attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets
        )
        .is_err());
        assert_eq!(fs::read(config_path).unwrap(), original);
        assert!(!backup_path(&backups).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attach_rejects_non_utf8_auth_without_rewriting_it() {
        let (root, home, backups) = profile_dirs("non-utf8-auth");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        let auth_path = home.join(AUTH_FILE);
        let original = vec![0xff, 0xfe, 0xfd];
        fs::write(&auth_path, &original).unwrap();

        assert!(attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &MemorySecrets::default()
        )
        .is_err());
        assert_eq!(fs::read(auth_path).unwrap(), original);
        assert!(!backup_path(&backups).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_login_during_attach_is_not_overwritten() {
        let (root, home, backups) = profile_dirs("external-login");
        let config_path = home.join(CONFIG_FILE);
        let auth_path = home.join(AUTH_FILE);
        let original_config = "model_provider = \"openai\"\n";
        let fresh_auth = b"{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}";
        fs::write(&config_path, original_config).unwrap();
        fs::write(&auth_path, "{\"auth_mode\":\"chatgpt\"}").unwrap();
        let secrets = MutatingSecrets::new(auth_path.clone(), fresh_auth.to_vec());

        let error = attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap_err();

        assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
        assert_eq!(fs::read_to_string(config_path).unwrap(), original_config);
        assert_eq!(fs::read(auth_path).unwrap(), fresh_auth);
        assert!(!backup_path(&backups).exists());
        assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_config_change_during_attach_is_not_overwritten() {
        let (root, home, backups) = profile_dirs("external-config");
        let config_path = home.join(CONFIG_FILE);
        let changed_config = b"model_provider = \"custom\"\n";
        fs::write(&config_path, "model_provider = \"openai\"\n").unwrap();
        fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"chatgpt\"}").unwrap();
        let secrets = MutatingSecrets::new(config_path.clone(), changed_config.to_vec());

        let error = attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap_err();

        assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
        assert_eq!(fs::read(config_path).unwrap(), changed_config);
        assert!(!backup_path(&backups).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_reattach_restores_previous_backup_metadata() {
        let (root, home, backups) = profile_dirs("backup-rollback");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_old_key",
            &secrets,
        )
        .unwrap();
        let managed_config = fs::read(home.join(CONFIG_FILE)).unwrap();
        let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();
        fs::create_dir(home.join("config.tmp")).unwrap();

        assert!(attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14999/v1",
            "zlr_new_key",
            &secrets,
        )
        .is_err());
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), managed_config);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), managed_auth);
        let pending: Value =
            serde_json::from_str(&fs::read_to_string(backup_path(&backups)).unwrap()).unwrap();
        assert_eq!(pending["restorePending"], true);
        fs::remove_dir_all(home.join("config.tmp")).unwrap();
        restore_with(&home, &backups, &secrets).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            "model_provider = \"openai\"\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_backup_secret_cleanup_rolls_restore_back() {
        let (root, home, backups) = profile_dirs("restore-cleanup-rollback");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"old\"}}",
        )
        .unwrap();
        let secrets = FailingDeleteSecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        assert!(restore_with(&home, &backups, &secrets).is_err());
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            "model_provider = \"openai\"\n"
        );
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("old"));
        let pending: Value =
            serde_json::from_str(&fs::read_to_string(backup_path(&backups)).unwrap()).unwrap();
        assert_eq!(pending["restorePending"], true);
        assert!(!managed_model_catalog_path(&backups).unwrap().exists());
        assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_backup_during_restore_rolls_profile_back() {
        let (root, home, backups) = profile_dirs("restore-backup-race");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"old\"}}",
        )
        .unwrap();
        let external_backup = b"external backup change".to_vec();
        let secrets = MutatingLoadSecrets::new(backup_path(&backups), external_backup.clone());
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        let managed_config = fs::read(home.join(CONFIG_FILE)).unwrap();
        let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();

        let error = restore_with(&home, &backups, &secrets).unwrap_err();

        assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), managed_config);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), managed_auth);
        assert_eq!(fs::read(backup_path(&backups)).unwrap(), external_backup);
        assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oauth_account_attach_reuses_one_profile_binding_and_restores_previous_login() {
        let (root, home, backups) = profile_dirs("oauth-account");
        let previous_config = r#"model_provider = "custom"
openai_base_url = "https://stale.example.com/v1"

[model_providers.custom]
name = "Custom"
base_url = "https://custom.example.com/v1"
"#;
        fs::write(home.join(CONFIG_FILE), previous_config).unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"previous\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        let first = TokenSet::new(
            "access-secret",
            Some("refresh-secret".into()),
            Some("id-secret".into()),
            Some(60_000),
            1,
            1,
        )
        .unwrap();
        let binding = attach_account_with(
            &home,
            &backups,
            "account-local",
            &first,
            "provider-private-id",
            &secrets,
        )
        .unwrap();
        assert_eq!(binding.credential_id, "account-local");
        let stored_bindings = account_bindings(&backups).unwrap();
        assert_eq!(stored_bindings.len(), 1);
        assert_eq!(stored_bindings[0].credential_id, binding.credential_id);
        assert!(profile_bindings(&home, &backups).unwrap()[0].active);
        let account_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
        assert!(!account_config.contains("model_provider ="));
        assert!(!account_config.contains("openai_base_url"));
        assert!(!account_config.contains("[model_providers.zenith_relay_local]"));
        assert!(account_config.contains("[model_providers.custom]"));
        let account_auth: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
        assert_eq!(account_auth["OPENAI_API_KEY"], serde_json::Value::Null);
        assert_eq!(account_auth["tokens"]["refresh_token"], "refresh-secret");
        assert!(account_auth.get("auth_mode").is_none());

        let canonical_home = canonical_profile_dir(&home).unwrap();
        let backup_path = account_backup_path(&backups, &canonical_home);
        let backup = fs::read_to_string(&backup_path).unwrap();
        for secret in [
            "access-secret",
            "refresh-secret",
            "id-secret",
            "provider-private-id",
        ] {
            assert!(!backup.contains(secret));
        }

        attach_account_with(
            &home,
            &backups,
            "account-local",
            &first,
            "provider-private-id",
            &secrets,
        )
        .unwrap();
        assert_eq!(account_bindings(&backups).unwrap().len(), 1);

        let refreshed = TokenSet::new(
            "access-refreshed",
            Some("refresh-new".into()),
            Some("id-new".into()),
            Some(120_000),
            2,
            2,
        )
        .unwrap();
        assert_eq!(
            sync_account_bindings(&backups, "account-local", &refreshed, "provider-private-id",)
                .unwrap(),
            1
        );
        assert_eq!(
            sync_account_bindings(&backups, "account-local", &refreshed, "provider-private-id",)
                .unwrap(),
            0
        );
        assert_eq!(account_bindings(&backups).unwrap().len(), 1);
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("access-refreshed"));

        let restored = restore_account_with(&home, &backups, &secrets)
            .unwrap()
            .unwrap();
        assert_eq!(restored, binding);
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            previous_config
        );
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("previous"));
        assert!(account_bindings(&backups).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_profile_rotation_is_adopted_only_for_the_same_account() {
        let (root, home, backups) = profile_dirs("managed-token-adoption");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
        let secrets = MemorySecrets::default();
        let original = TokenSet::new(
            "access-original",
            Some("refresh-original".into()),
            Some("id-original".into()),
            Some(60_000),
            1,
            1,
        )
        .unwrap();
        attach_account_with(
            &home,
            &backups,
            "account-local",
            &original,
            "provider-account",
            &secrets,
        )
        .unwrap();

        let rotated = TokenSet::new(
            "access-rotated",
            Some("refresh-rotated".into()),
            Some("id-rotated".into()),
            Some(120_000),
            2,
            2,
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            account_auth_content(&rotated, "provider-account").unwrap(),
        )
        .unwrap();
        let update = managed_account_token_update(
            &home,
            &backups,
            "account-local",
            original.access_token(),
            "provider-account",
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.access_token, "access-rotated");
        assert_eq!(update.refresh_token, "refresh-rotated");
        assert_eq!(update.id_token.as_deref(), Some("id-rotated"));
        let debug = format!("{update:?}");
        assert!(!debug.contains("rotated"));

        assert_eq!(
            sync_account_bindings(&backups, "account-local", &rotated, "provider-account").unwrap(),
            1
        );
        assert!(managed_account_token_update(
            &home,
            &backups,
            "account-local",
            rotated.access_token(),
            "provider-account",
        )
        .unwrap()
        .is_none());

        let other = TokenSet::new(
            "other-access",
            Some("other-refresh".into()),
            Some("other-id".into()),
            Some(180_000),
            3,
            3,
        )
        .unwrap();
        fs::write(
            home.join(AUTH_FILE),
            account_auth_content(&other, "provider-other").unwrap(),
        )
        .unwrap();
        assert!(managed_account_token_update(
            &home,
            &backups,
            "account-local",
            rotated.access_token(),
            "provider-account",
        )
        .unwrap()
        .is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn switching_account_and_local_gateway_preserves_the_original_profile() {
        let (root, home, backups) = profile_dirs("credential-kind-switch");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new(
            "managed-account",
            Some("refresh".into()),
            Some("id-token".into()),
            Some(60_000),
            1,
            1,
        )
        .unwrap();

        let local = switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions::default(),
            &secrets,
        )
        .unwrap();
        assert_eq!(local.credential_kind, ProfileCredentialKind::LocalGateway);
        assert_eq!(profile_bindings(&home, &backups).unwrap(), vec![local]);
        assert_eq!(profile_backup_count(&backups), 1);

        let account = switch_to_account_with(
            &home,
            &backups,
            "account-local",
            &tokens,
            "provider-account",
            &secrets,
        )
        .unwrap();
        assert_eq!(account.credential_kind, ProfileCredentialKind::OAuthAccount);
        assert_eq!(profile_bindings(&home, &backups).unwrap(), vec![account]);
        assert!(!backup_path(&backups).exists());
        assert_eq!(profile_backup_count(&backups), 1);

        switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions::default(),
            &secrets,
        )
        .unwrap();
        assert_eq!(profile_backup_count(&backups), 1);
        restore_with(&home, &backups, &secrets).unwrap();

        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .contains("model_provider = \"custom\""));
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("original"));
        assert_eq!(profile_backup_count(&backups), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_binding_detects_an_external_provider_takeover() {
        let (root, home, backups) = profile_dirs("external-provider-active-state");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"apikey\"}").unwrap();
        let secrets = MemorySecrets::default();
        switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions::default(),
            &secrets,
        )
        .unwrap();
        assert!(profile_bindings(&home, &backups).unwrap()[0].active);

        let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();
        fs::write(home.join(AUTH_FILE), r#"{"auth_mode":"apikey"}"#).unwrap();
        assert!(!profile_bindings(&home, &backups).unwrap()[0].active);
        fs::write(home.join(AUTH_FILE), managed_auth).unwrap();

        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"codex_local_access\"\n\n[model_providers.codex_local_access]\nbase_url = \"https://api.example.test/v1\"\n",
        )
        .unwrap();
        let bindings = profile_bindings(&home, &backups).unwrap();
        assert_eq!(bindings.len(), 1);
        assert!(!bindings[0].active);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn switching_external_account_takeover_to_local_rebases_the_latest_profile() {
        let (root, home, backups) = profile_dirs("external-account-takeover");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
        fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"chatgpt\"}").unwrap();
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new(
            "managed-access",
            Some("managed-refresh".into()),
            Some("managed-id".into()),
            Some(60_000),
            1,
            1,
        )
        .unwrap();
        attach_account_with(
            &home,
            &backups,
            "account-local",
            &tokens,
            "provider-account",
            &secrets,
        )
        .unwrap();

        let external_config = "model_provider = \"codex_local_access\"\n\n[model_providers.codex_local_access]\nname = \"Codex API Service\"\nbase_url = \"http://127.0.0.1:49976/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n";
        let external_auth = "{\"tokens\":{\"access_token\":\"external\"}}";
        fs::write(home.join(CONFIG_FILE), external_config).unwrap();
        fs::write(home.join(AUTH_FILE), external_auth).unwrap();

        switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions {
                bound_oauth: Some(BoundOAuthProfile {
                    account_id: "account-local",
                    tokens: &tokens,
                    provider_account_id: "provider-account",
                }),
                ..LocalAttachOptions::default()
            },
            &secrets,
        )
        .unwrap();
        assert_eq!(profile_backup_count(&backups), 1);
        assert!(backup_path(&backups).exists());

        restore_with(&home, &backups, &secrets).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            external_config
        );
        assert_eq!(
            fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
            external_auth
        );
        assert_eq!(profile_backup_count(&backups), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_gateway_projects_and_syncs_a_bound_oauth_profile() {
        let (root, home, backups) = profile_dirs("local-gateway-bound-oauth");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new(
            "bound-access",
            Some("bound-refresh".into()),
            Some("bound-id".into()),
            Some(60_000),
            1,
            1,
        )
        .unwrap();

        let binding = switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions {
                bound_oauth: Some(BoundOAuthProfile {
                    account_id: "account-local",
                    tokens: &tokens,
                    provider_account_id: "provider-account",
                }),
                ..LocalAttachOptions::default()
            },
            &secrets,
        )
        .unwrap();
        assert_eq!(
            binding.bound_oauth_account_id.as_deref(),
            Some("account-local")
        );
        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .contains("model_provider = \"zenith_relay_local\""));
        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .contains("experimental_bearer_token = \"zlr_key\""));
        let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
        assert!(projected.contains("bound-access"));
        assert!(!projected.contains("zlr_key"));
        let projected_value = serde_json::from_str::<serde_json::Value>(&projected).unwrap();
        assert!(projected_value["OPENAI_API_KEY"].is_null());
        assert!(projected_value.get("auth_mode").is_none());
        assert_eq!(projected_value["tokens"]["account_id"], "provider-account");
        DateTime::parse_from_rfc3339(projected_value["last_refresh"].as_str().unwrap()).unwrap();

        let refreshed = TokenSet::new(
            "bound-access-refreshed",
            Some("bound-refresh-next".into()),
            Some("bound-id-next".into()),
            Some(120_000),
            2,
            2,
        )
        .unwrap();
        assert!(sync_local_gateway_binding(
            &home,
            &backups,
            "account-local",
            &refreshed,
            "provider-account",
        )
        .unwrap());
        assert!(!sync_local_gateway_binding(
            &home,
            &backups,
            "account-local",
            &refreshed,
            "provider-account",
        )
        .unwrap());
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("bound-access-refreshed"));

        restore_with(&home, &backups, &secrets).unwrap();
        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .contains("model_provider = \"custom\""));
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("original"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_gateway_can_replace_bound_oauth_with_local_key() {
        let (root, home, backups) = profile_dirs("local-gateway-remove-oauth-binding");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new(
            "bound-access",
            Some("bound-refresh".into()),
            Some("bound-id".into()),
            Some(60_000),
            1,
            1,
        )
        .unwrap();

        switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions {
                bound_oauth: Some(BoundOAuthProfile {
                    account_id: "account-local",
                    tokens: &tokens,
                    provider_account_id: "provider-account",
                }),
                ..LocalAttachOptions::default()
            },
            &secrets,
        )
        .unwrap();
        let binding = switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions::default(),
            &secrets,
        )
        .unwrap();

        assert_eq!(binding.bound_oauth_account_id, None);
        let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
        assert!(projected.contains("zlr_key"));
        assert!(!projected.contains("bound-access"));
        restore_with(&home, &backups, &secrets).unwrap();
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("original"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_gateway_keeps_api_key_projection_when_bound_oauth_has_no_id_token() {
        let (root, home, backups) = profile_dirs("local-gateway-bound-access-only");
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new(
            "bound-access",
            Some("bound-refresh".into()),
            None,
            Some(60_000),
            1,
            1,
        )
        .unwrap();

        let binding = switch_to_local_with(
            &home,
            &backups,
            "key-local",
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            LocalAttachOptions {
                bound_oauth: Some(BoundOAuthProfile {
                    account_id: "account-local",
                    tokens: &tokens,
                    provider_account_id: "provider-account",
                }),
                ..LocalAttachOptions::default()
            },
            &secrets,
        )
        .unwrap();
        assert_eq!(
            binding.bound_oauth_account_id.as_deref(),
            Some("account-local")
        );
        let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
        assert!(projected.contains("zlr_key"));
        assert!(!projected.contains("bound-access"));
        restore_with(&home, &backups, &secrets).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oauth_account_restore_refuses_a_fresh_manual_login() {
        let (root, home, backups) = profile_dirs("oauth-fresh-login");
        fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
        attach_account_with(
            &home,
            &backups,
            "account-local",
            &tokens,
            "provider-private-id",
            &secrets,
        )
        .unwrap();
        let auth: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
        assert_eq!(auth["tokens"]["refresh_token"], "");
        fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
        )
        .unwrap();

        assert!(matches!(
            restore_account_with(&home, &backups, &secrets)
                .unwrap_err()
                .code,
            ErrorCode::ProfileRestoreBlocked
        ));
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("fresh"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oauth_account_bindings_are_isolated_per_profile_path() {
        let (root, first, backups) = profile_dirs("oauth-multi-profile");
        let second = root.join("second-profile");
        fs::create_dir_all(&second).unwrap();
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
        attach_account_with(
            &first,
            &backups,
            "account-local",
            &tokens,
            "provider-private-id",
            &secrets,
        )
        .unwrap();
        attach_account_with(
            &second,
            &backups,
            "account-local",
            &tokens,
            "provider-private-id",
            &secrets,
        )
        .unwrap();
        assert_eq!(account_bindings(&backups).unwrap().len(), 2);

        restore_account_with(&first, &backups, &secrets).unwrap();
        let remaining = account_bindings(&backups).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].profile_dir,
            canonical_profile_dir(&second).unwrap().to_string_lossy()
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn profile_dirs(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-profile-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        let home = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&home).unwrap();
        (root, home, backups)
    }

    fn profile_backup_count(backups: &Path) -> usize {
        fs::read_dir(backups)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .count()
    }

    fn write_test_catalog_file(path: &Path, slug: &str) {
        let mut entry = routed_codex_catalog_entry(None, slug, 2, None);
        entry["slug"] = Value::String(slug.into());
        entry["display_name"] = Value::String(slug.into());
        entry["description"] = Value::String("Native user model".into());
        entry["comp_hash"] = Value::String("official".into());
        entry["default_reasoning_level"] = Value::String("medium".into());
        entry["supported_reasoning_levels"] = json!([
            {"effort": "medium", "description": "Medium"}
        ]);
        fs::write(
            path,
            serde_json::to_string_pretty(&json!({"models": [entry]})).unwrap(),
        )
        .unwrap();
    }
}

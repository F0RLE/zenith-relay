use crate::{
    codex_config::lock_codex_profile,
    files::{atomic_write, escape_json_string},
    local_pool::{
        error::{ErrorCode, LocalPoolError, Result},
        store::secret_store,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{value, DocumentMut, Item, Table};
use zenith_relay_core::accounts::TokenSet;

const PROVIDER_ID: &str = "zenith_relay_local";
const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
const BACKUP_SECRET_REF: &str = "profile:codex:default:previous_auth";
const ACCOUNT_BACKUP_PREFIX: &str = "codex-account-";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileBackup {
    version: u32,
    previous_model_provider: Option<String>,
    previous_auth_secret_ref: Option<String>,
    managed_key_hash: String,
    managed_base_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountProfileBackup {
    version: u32,
    profile_dir: String,
    previous_model_provider: Option<String>,
    previous_auth_secret_ref: Option<String>,
    managed_account_id: String,
    managed_access_hash: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileCredentialKind {
    OAuthAccount,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBinding {
    pub profile_dir: String,
    pub credential_kind: ProfileCredentialKind,
    pub credential_id: String,
}

pub fn attach(
    codex_home: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
) -> Result<()> {
    attach_with(
        codex_home,
        backup_root,
        base_url,
        local_key,
        &OsSecretBackend,
    )
}

pub fn restore(codex_home: &Path, backup_root: &Path) -> Result<()> {
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        restore_account_profile(codex_home, backup_root)?;
        return Ok(());
    }
    restore_with(codex_home, backup_root, &OsSecretBackend)
}

pub fn attach_account(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<ProfileBinding> {
    attach_account_with(
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
    restore_account_with(codex_home, backup_root, &OsSecretBackend)
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
        bindings.push(binding_from_backup(&backup));
    }
    bindings.sort_by(|left, right| left.profile_dir.cmp(&right.profile_dir));
    Ok(bindings)
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

fn attach_with(
    codex_home: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    let local_key = local_key.trim();
    let base_url = base_url.trim_end_matches('/');
    if local_key.is_empty() || base_url.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "profile base URL and local key are required",
        ));
    }
    fs::create_dir_all(codex_home).map_err(io_error)?;
    fs::create_dir_all(backup_root).map_err(io_error)?;
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let backup_path = backup_path(backup_root);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_backup_bytes = read_optional_bytes(&backup_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let original_auth = snapshot_text(&original_auth_bytes, &auth_path)?;
    let mut document = parse_config(original_config)?;
    validate_config_shape(&document)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Codex profile is already attached to an OAuth account",
        ));
    }
    let existing_backup = parse_backup_snapshot(&original_backup_bytes, &backup_path)?;

    if existing_backup.is_none() && document_has_provider(&document) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed Codex provider exists without a profile backup",
        ));
    }
    if existing_backup.is_some()
        && !managed_config_matches(&document, existing_backup.as_ref().unwrap())
    {
        return Err(profile_restore_blocked());
    }
    if let Some(backup) = existing_backup.as_ref() {
        if !auth_matches_snapshot(&original_auth_bytes, &auth_path, &backup.managed_key_hash)? {
            return Err(profile_restore_blocked());
        }
    }

    let created_backup = existing_backup.is_none();
    let mut backup = existing_backup.unwrap_or(ProfileBackup {
        version: 1,
        previous_model_provider: root_model_provider(&document),
        previous_auth_secret_ref: None,
        managed_key_hash: String::new(),
        managed_base_url: String::new(),
    });
    if created_backup {
        if let Some(previous_auth) = original_auth.filter(|value| !value.trim().is_empty()) {
            secrets.save(BACKUP_SECRET_REF, previous_auth)?;
            backup.previous_auth_secret_ref = Some(BACKUP_SECRET_REF.to_string());
        }
    }
    backup.managed_key_hash = key_hash(local_key);
    backup.managed_base_url = base_url.to_string();
    let backup_content = serialize_backup(&backup)?;
    if let Err(error) = replace_if_unchanged(&backup_path, &original_backup_bytes, &backup_content)
    {
        return Err(with_rollback(
            error,
            cleanup_created_backup_secret(created_backup, &backup, secrets),
        ));
    }

    attach_config(&mut document, base_url);
    let managed_config = document.to_string();
    if let Err(error) = replace_if_unchanged(&config_path, &original_config_bytes, &managed_config)
    {
        return Err(with_rollback(
            error,
            rollback_backup(
                created_backup,
                &backup_path,
                &backup_content,
                &original_backup_bytes,
                &backup,
                secrets,
            ),
        ));
    }
    let managed_auth = auth_content(local_key);
    if let Err(error) = replace_if_unchanged(&auth_path, &original_auth_bytes, &managed_auth) {
        let config_rollback = rollback_file(&config_path, &managed_config, &original_config_bytes);
        let backup_rollback = rollback_backup(
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
    Ok(())
}

fn restore_with(codex_home: &Path, backup_root: &Path, secrets: &impl SecretBackend) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    let backup_path = backup_path(backup_root);
    let backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(backup) = parse_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(());
    };
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let mut document = parse_config(original_config)?;
    if !managed_config_matches(&document, &backup)
        || !auth_matches_snapshot(&original_auth_bytes, &auth_path, &backup.managed_key_hash)?
    {
        return Err(profile_restore_blocked());
    }
    let previous_auth = match backup.previous_auth_secret_ref.as_deref() {
        Some(secret_ref) => Some(secrets.load(secret_ref)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "Codex profile backup secret is missing",
            )
        })?),
        None => None,
    };
    restore_config(&mut document, backup.previous_model_provider.as_deref());
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
    Ok(())
}

fn attach_account_with(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let _profile_guard = lock_codex_profile();
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
        previous_auth_secret_ref: None,
        managed_account_id: String::new(),
        managed_access_hash: String::new(),
    });
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
    Ok(binding_from_backup(&backup))
}

fn restore_account_with(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<Option<ProfileBinding>> {
    let _profile_guard = lock_codex_profile();
    let profile_dir = canonical_profile_dir(codex_home)?;
    let backup_path = account_backup_path(backup_root, &profile_dir);
    let backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(backup) = parse_account_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(None);
    };
    if backup.profile_dir != profile_dir.to_string_lossy() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex account profile backup points to another profile",
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
                "Codex account profile backup secret is missing",
            )
        })?),
        None => None,
    };
    restore_config(&mut document, backup.previous_model_provider.as_deref());
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
    Ok(Some(binding_from_backup(&backup)))
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
    if !account_managed_config_matches(&document)
        || !account_auth_matches_snapshot(&auth, &auth_path, &backup.managed_access_hash)?
    {
        return Ok(false);
    }
    backup.managed_access_hash = next_hash;
    let updated_backup = serialize_account_backup(&backup)?;
    replace_if_unchanged(&backup_path, &backup_bytes, &updated_backup)?;
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
    document["model_provider"] = value("openai");
}

fn account_managed_config_matches(document: &DocumentMut) -> bool {
    root_model_provider(document).as_deref() == Some("openai") && !document_has_provider(document)
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
    if let Some(refresh_token) = tokens.refresh_token() {
        token_values.insert(
            "refresh_token".into(),
            serde_json::Value::String(refresh_token.to_string()),
        );
    }
    if let Some(id_token) = tokens.id_token() {
        token_values.insert(
            "id_token".into(),
            serde_json::Value::String(id_token.to_string()),
        );
    }
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "auth_mode": "chatgpt",
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
    Ok(value
        .get("auth_mode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode == "chatgpt")
        && value
            .get("tokens")
            .and_then(|tokens| tokens.get("access_token"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| key_hash(token.trim()) == expected_hash))
}

fn account_backup_for_profile(codex_home: &Path, backup_root: &Path) -> Result<Option<PathBuf>> {
    if !codex_home.exists() {
        return Ok(None);
    }
    let path = account_backup_path(backup_root, &canonical_profile_dir(codex_home)?);
    Ok(path.exists().then_some(path))
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
            "Codex profile path is not a directory",
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
                "Codex account profile backup is invalid at {}: {error}",
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
            "Codex account profile backup has invalid metadata",
        ));
    }
    Ok(backup)
}

fn serialize_account_backup(backup: &AccountProfileBackup) -> Result<String> {
    let content = serde_json::to_string_pretty(backup)
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    Ok(format!("{content}\n"))
}

fn binding_from_backup(backup: &AccountProfileBackup) -> ProfileBinding {
    ProfileBinding {
        profile_dir: backup.profile_dir.clone(),
        credential_kind: ProfileCredentialKind::OAuthAccount,
        credential_id: backup.managed_account_id.clone(),
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
            format!("Codex config is not valid TOML: {error}"),
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
            "Codex model_provider must be a string",
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
            "Codex model_providers must be a table",
        ));
    }
    Ok(())
}

fn attach_config(document: &mut DocumentMut, base_url: &str) {
    document["model_provider"] = value(PROVIDER_ID);
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
    provider["supports_websockets"] = value(false);
}

fn restore_config(document: &mut DocumentMut, previous_model_provider: Option<&str>) {
    if let Some(model_providers) = document["model_providers"].as_table_mut() {
        model_providers.remove(PROVIDER_ID);
        if model_providers.is_empty() {
            document.remove("model_providers");
        }
    }
    if let Some(previous_model_provider) = previous_model_provider {
        document["model_provider"] = value(previous_model_provider);
    } else {
        document.remove("model_provider");
    }
}

fn root_model_provider(document: &DocumentMut) -> Option<String> {
    document
        .get("model_provider")
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
    document
        .get("model_provider")
        .and_then(Item::as_str)
        .is_some_and(|provider| provider == PROVIDER_ID)
        && document
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
                    && provider.get("supports_websockets").and_then(Item::as_bool) == Some(false)
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

fn key_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn backup_path(root: &Path) -> std::path::PathBuf {
    root.join("codex-default.json")
}

fn parse_backup_snapshot(snapshot: &Option<Vec<u8>>, path: &Path) -> Result<Option<ProfileBackup>> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(None);
    };
    serde_json::from_str(content).map(Some).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("Codex profile backup is invalid: {error}"),
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
        "Codex profile changed after attach; restore was not applied",
    )
}

fn profile_changed_at(path: &Path) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        format!(
            "Codex changed {} while Zenith Relay was updating the profile; no replacement was applied",
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
        let previous_backup = fs::read(backup_path(&backups)).unwrap();
        let previous_config = fs::read(home.join(CONFIG_FILE)).unwrap();
        let previous_auth = fs::read(home.join(AUTH_FILE)).unwrap();
        fs::create_dir(home.join("config.tmp")).unwrap();

        assert!(attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14999/v1",
            "zlr_new_key",
            &secrets,
        )
        .is_err());
        assert_eq!(fs::read(backup_path(&backups)).unwrap(), previous_backup);
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), previous_config);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), previous_auth);
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
        let managed_config = fs::read(home.join(CONFIG_FILE)).unwrap();
        let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();
        let managed_backup = fs::read(backup_path(&backups)).unwrap();

        assert!(restore_with(&home, &backups, &secrets).is_err());
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), managed_config);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), managed_auth);
        assert_eq!(fs::read(backup_path(&backups)).unwrap(), managed_backup);
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
        fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
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
        assert_eq!(account_bindings(&backups).unwrap(), vec![binding.clone()]);

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
        assert!(fs::read_to_string(home.join(CONFIG_FILE))
            .unwrap()
            .contains("model_provider = \"custom\""));
        assert!(fs::read_to_string(home.join(AUTH_FILE))
            .unwrap()
            .contains("previous"));
        assert!(account_bindings(&backups).unwrap().is_empty());
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
}

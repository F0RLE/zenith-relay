use super::*;
use std::{fs, path::Path};
use zenith_relay_core::accounts::TokenSet;

pub(super) fn attach_account_locked(
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

pub(super) fn restore_account_locked(
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

pub(super) fn sync_account_profile_with(
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

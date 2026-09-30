use super::*;

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

/// A user-requested activation may first detach an old binding without
/// replacing a login made independently in Codex, then back up that login as
/// the baseline for the new binding. Do not use this for automatic rollback.
pub fn attach_account_explicit(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<ProfileBinding> {
    switch_to_account_with_intent(
        codex_home,
        backup_root,
        account_id,
        tokens,
        provider_account_id,
        true,
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

pub(in crate::local_pool::profiles) fn snapshot_user_profile(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<UserProfileSnapshot> {
    snapshot_user_profile_with(codex_home, backup_root, &OsSecretBackend)
}

pub(super) fn snapshot_user_profile_with(
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
            if let Some(secret_ref) = backup.projection_secret_ref.as_deref() {
                let mut restored = projection::restore(
                    secret_ref,
                    snapshot_text(&config, &config_path)?,
                    snapshot_text(&auth, &auth_path)?,
                    secrets,
                )?;
                if !account_auth_matches_snapshot(&auth, &auth_path, &backup.managed_access_hash)? {
                    restored.auth = snapshot_text(&auth, &auth_path)?.map(str::to_owned);
                }
                return Ok(restored);
            }
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
            if let Some(secret_ref) = backup.projection_secret_ref.as_deref() {
                let mut restored = projection::restore(
                    secret_ref,
                    snapshot_text(&config, &config_path)?,
                    snapshot_text(&auth, &auth_path)?,
                    secrets,
                )?;
                if !managed_auth_matches_snapshot(&auth, &auth_path, &backup)? {
                    restored.auth = snapshot_text(&auth, &auth_path)?.map(str::to_owned);
                }
                return Ok(restored);
            }
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
            remove_managed_provider(&mut document, &backup.managed_provider_id);
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

pub(in crate::local_pool::profiles) fn restore_full_user_profile_snapshot(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
) -> Result<()> {
    restore_user_profile_snapshot_full_with(codex_home, backup_root, snapshot, &OsSecretBackend)
}

pub(super) fn restore_user_profile_snapshot_full_with(
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

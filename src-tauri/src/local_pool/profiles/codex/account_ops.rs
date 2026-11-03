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
    switch_transaction::run(codex_home, &OsSecretBackend, |secrets| {
        account::restore_account_locked(codex_home, backup_root, secrets)
    })
}

#[cfg(test)]
pub(in crate::local_pool::profiles) fn snapshot_user_profile(
    codex_home: &Path,
) -> Result<UserProfileSnapshot> {
    let _profile_guard = lock_codex_profile();
    snapshot_user_profile_locked(codex_home, &snapshot_credentials::NativeAuthKeyring)
}

pub(super) fn snapshot_user_profile_locked(
    codex_home: &Path,
    keyring: &impl snapshot_credentials::AuthKeyring,
) -> Result<UserProfileSnapshot> {
    fs::create_dir_all(codex_home).map_err(io_error)?;
    let profile_dir = canonical_profile_dir(codex_home)?;
    let _auth_lock = snapshot_credentials::lock(&profile_dir)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let config_bytes = read_optional_bytes(&config_path)?;
    let auth_bytes = read_optional_bytes(&auth_path)?;
    // A named snapshot records the connection visible at capture time. The
    // automatic attachment undo record is a different recovery operation.
    let config = snapshot_text(&config_bytes, &config_path)?.map(str::to_string);
    let auth = snapshot_credentials::read(
        &profile_dir,
        config.as_deref(),
        snapshot_text(&auth_bytes, &auth_path)?,
        keyring,
    )?;
    Ok(UserProfileSnapshot {
        config: snapshot_credentials::pin_backend(config)?,
        auth,
    })
}

pub(in crate::local_pool::profiles) struct CapturedSnapshot {
    pub profile: UserProfileSnapshot,
    pub binding: Option<ManagedBindingSnapshot>,
    pub catalog: Option<String>,
}

pub(in crate::local_pool::profiles) fn capture_user_profile_snapshot(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<CapturedSnapshot> {
    let _profile_guard = lock_codex_profile();
    let profile =
        snapshot_user_profile_locked(codex_home, &snapshot_credentials::NativeAuthKeyring)?;
    let home = canonical_profile_dir(codex_home)?;
    let binding = ManagedBindingSnapshot::capture(&home, backup_root, &profile, secrets)?;
    let catalog = snapshot::capture_managed_catalog(backup_root, &profile)?;
    Ok(CapturedSnapshot {
        profile,
        binding,
        catalog,
    })
}

pub(in crate::local_pool::profiles) fn restore_full_user_profile_snapshot(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
    managed_model_catalog: Option<&str>,
    managed_binding: Option<&ManagedBindingSnapshot>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    restore_user_profile_snapshot_full_with(
        codex_home,
        backup_root,
        snapshot,
        managed_model_catalog,
        managed_binding,
        secrets,
    )
}

pub(super) fn restore_user_profile_snapshot_full_with(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
    managed_model_catalog: Option<&str>,
    managed_binding: Option<&ManagedBindingSnapshot>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    restore_snapshot_with_keyring(
        codex_home,
        backup_root,
        snapshot,
        managed_model_catalog,
        managed_binding,
        secrets,
        &snapshot_credentials::NativeAuthKeyring,
    )
}

pub(super) fn restore_snapshot_with_keyring(
    codex_home: &Path,
    backup_root: &Path,
    snapshot: &UserProfileSnapshot,
    managed_model_catalog: Option<&str>,
    managed_binding: Option<&ManagedBindingSnapshot>,
    secrets: &impl SecretBackend,
    keyring: &impl snapshot_credentials::AuthKeyring,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    fs::create_dir_all(codex_home).map_err(io_error)?;
    ensure_single_profile_backup(codex_home, backup_root)?;
    let profile_dir = canonical_profile_dir(codex_home)?;
    let _auth_lock = snapshot_credentials::lock(&profile_dir)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let current_config = snapshot_text(&original_config_bytes, &config_path)?;
    let ephemeral = parse_config(current_config.unwrap_or_default())?
        .get("cli_auth_credentials_store")
        .and_then(Item::as_str)
        == Some("ephemeral");
    let current_auth = if ephemeral {
        None
    } else {
        snapshot_credentials::read(
            &profile_dir,
            current_config,
            snapshot_text(&original_auth_bytes, &auth_path)?,
            keyring,
        )?
    };
    let snapshot = snapshot::restore_connection(
        snapshot,
        snapshot_text(&original_config_bytes, &config_path)?,
        current_auth.as_deref(),
    )?;

    fs::create_dir_all(backup_root).map_err(io_error)?;
    let catalog_path = managed_model_catalog_path(backup_root)?;
    let preserve_catalog = snapshot::references_catalog(snapshot.config.as_deref(), &catalog_path)?;
    let auth_store = snapshot_credentials::store(snapshot.config.as_deref())?;
    let encrypted = snapshot_credentials::encrypted_backend(snapshot.config.as_deref())?;
    let original_keyring = if auth_store != snapshot_credentials::AuthStore::File {
        Some(keyring.load(&profile_dir, encrypted)?)
    } else {
        None
    };
    if let Some(catalog) = managed_model_catalog {
        snapshot::validate_snapshot_catalog(catalog)?;
        if !preserve_catalog {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "Snapshot model catalog does not match its connection",
            ));
        }
    }
    let restore_secrets = snapshot_credentials::RestoreSecrets {
        home: &profile_dir,
        encrypted,
        secrets,
        keyring,
    };
    switch_transaction::run(&profile_dir, &restore_secrets, |secrets| {
        if let Some(catalog) = managed_model_catalog {
            let previous = read_optional_bytes(&catalog_path)?;
            replace_with_snapshot(&catalog_path, &previous, Some(catalog))?;
        }
        replace_with_snapshot(
            &config_path,
            &original_config_bytes,
            snapshot.config.as_deref(),
        )?;
        if auth_store == snapshot_credentials::AuthStore::File {
            replace_with_snapshot(&auth_path, &original_auth_bytes, snapshot.auth.as_deref())?;
        } else {
            if Some(secrets.load(snapshot_credentials::NATIVE_AUTH_REF)?) != original_keyring {
                return Err(profile_restore_blocked());
            }
            match snapshot.auth.as_deref() {
                Some(auth) => secrets.save(snapshot_credentials::NATIVE_AUTH_REF, auth)?,
                None => secrets.delete(snapshot_credentials::NATIVE_AUTH_REF)?,
            }
            replace_with_snapshot(&auth_path, &original_auth_bytes, None)?;
        }
        discard_managed_binding_locked(&profile_dir, backup_root, preserve_catalog, secrets)?;
        if let Some(binding) = managed_binding {
            binding.install(&profile_dir, backup_root, &snapshot, secrets)?;
        }
        let cache_path = profile_dir.join(MODELS_CACHE_FILE);
        let cache = read_optional_bytes(&cache_path)?;
        remove_if_unchanged(&cache_path, &cache)?;
        Ok(())
    })
}

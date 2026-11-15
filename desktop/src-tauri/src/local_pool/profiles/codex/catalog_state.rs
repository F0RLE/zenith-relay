use super::*;

pub(super) fn reconcile_pending_catalog_state(
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

pub(super) fn valid_managed_model_catalog(
    backup: &ProfileBackup,
    expected_catalog_path: &Path,
    catalog_bytes: &Option<Vec<u8>>,
) -> bool {
    if !valid_managed_model_catalog_metadata(backup, expected_catalog_path, catalog_bytes) {
        return false;
    }
    if backup.managed_model_catalog_path.is_none() {
        return true;
    }
    if backup.restore_pending && catalog_bytes.is_none() {
        return true;
    }
    let current_hash = catalog_bytes.as_deref().map(bytes_hash);
    let stable_valid = match backup.managed_model_catalog_hash.as_deref() {
        Some(hash) => current_hash.as_deref() == Some(hash),
        None => catalog_bytes.is_none(),
    };
    let pending_valid = backup
        .managed_model_catalog_pending_hash
        .as_deref()
        .is_some_and(|hash| current_hash.as_deref() == Some(hash));
    let pending_remove_valid =
        backup.managed_model_catalog_pending_remove && catalog_bytes.is_none();
    stable_valid || pending_valid || pending_remove_valid
}

fn valid_managed_model_catalog_metadata(
    backup: &ProfileBackup,
    expected_catalog_path: &Path,
    catalog_bytes: &Option<Vec<u8>>,
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
    let Some(recorded_catalog_path) = backup.managed_model_catalog_path.as_deref() else {
        return backup.managed_model_catalog_hash.is_none()
            && backup.managed_model_catalog_pending_hash.is_none()
            && !backup.managed_model_catalog_pending_remove
            && catalog_bytes.is_none();
    };
    if portable_path_value(recorded_catalog_path) != portable_path_string(expected_catalog_path) {
        return false;
    }
    true
}

/// A known Relay catalog path contains bytes not written by the current or
/// interrupted attach. It is not a corrupt backup: restore may leave the file
/// alone, while attach/refresh must still refuse to replace it.
pub(super) fn externally_changed_managed_model_catalog(
    backup: &ProfileBackup,
    expected_catalog_path: &Path,
    catalog_bytes: &Option<Vec<u8>>,
) -> bool {
    if !valid_managed_model_catalog_metadata(backup, expected_catalog_path, catalog_bytes)
        || backup.managed_model_catalog_path.is_none()
        || (backup.managed_model_catalog_hash.is_none()
            && backup.managed_model_catalog_pending_hash.is_none())
    {
        return false;
    }
    let Some(catalog_bytes) = catalog_bytes.as_deref() else {
        return false;
    };
    let current_hash = bytes_hash(catalog_bytes);
    backup.managed_model_catalog_hash.as_deref() != Some(current_hash.as_str())
        && backup.managed_model_catalog_pending_hash.as_deref() != Some(current_hash.as_str())
}

pub(super) fn managed_model_catalog_path(backup_root: &Path) -> Result<PathBuf> {
    let canonical_backup_root = fs::canonicalize(backup_root).map_err(io_error)?;
    Ok(canonical_backup_root.join(MODEL_CATALOG_FILE))
}

pub(super) fn apply_model_catalog_change(
    catalog_path: &Path,
    previous_catalog_snapshot: &Option<Vec<u8>>,
    updated_catalog_text: Option<&str>,
    previously_managed: bool,
) -> Result<()> {
    match (updated_catalog_text, previously_managed) {
        (Some(catalog_text), _) => {
            replace_if_unchanged(catalog_path, previous_catalog_snapshot, catalog_text)
        }
        (None, true) => remove_if_unchanged(catalog_path, previous_catalog_snapshot),
        (None, false) => Ok(()),
    }
}

pub(super) fn rollback_model_catalog_change(
    catalog_path: &Path,
    attempted_catalog_text: Option<&str>,
    previously_managed: bool,
    previous_catalog_snapshot: &Option<Vec<u8>>,
) -> Result<()> {
    match (attempted_catalog_text, previously_managed) {
        (Some(catalog_text), _) => {
            rollback_file(catalog_path, catalog_text, previous_catalog_snapshot)
        }
        (None, true) => {
            restore_snapshot_if_unchanged(catalog_path, &None, previous_catalog_snapshot)
        }
        (None, false) => Ok(()),
    }
}

pub(super) fn remove_managed_model_catalog_if_unchanged(backup: &ProfileBackup) {
    let Some(catalog_path) = backup.managed_model_catalog_path.as_deref() else {
        return;
    };
    let catalog_path = Path::new(catalog_path);
    let Ok(Some(catalog_bytes)) = read_optional_bytes(catalog_path) else {
        return;
    };
    let current_hash = bytes_hash(&catalog_bytes);
    if backup.managed_model_catalog_hash.as_deref() == Some(current_hash.as_str())
        || backup.managed_model_catalog_pending_hash.as_deref() == Some(current_hash.as_str())
    {
        let _ = remove_if_unchanged(catalog_path, &Some(catalog_bytes));
    }
}

pub(super) fn invalidate_models_cache(codex_home: &Path) -> Result<bool> {
    let cache_path = codex_home.join(MODELS_CACHE_FILE);
    let cache_snapshot = read_optional_bytes(&cache_path)?;
    if cache_snapshot.is_none() {
        return Ok(false);
    }
    remove_if_unchanged(&cache_path, &cache_snapshot)?;
    Ok(true)
}

pub(super) fn backup_path(backup_root: &Path) -> PathBuf {
    backup_root.join("codex-default.json")
}

pub(super) fn local_backup(codex_home: &Path, backup_root: &Path) -> Result<Option<ProfileBackup>> {
    let backup_path = backup_path(backup_root);
    let mut backup_snapshot = read_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_backup_snapshot(&backup_snapshot, &backup_path)? else {
        return Ok(None);
    };
    let catalog_path = managed_model_catalog_path(backup_root)?;
    let catalog_bytes = read_optional_bytes(&catalog_path)?;
    migrate_legacy_managed_catalog_metadata(
        codex_home,
        &backup_path,
        &mut backup_snapshot,
        &mut backup,
        &catalog_path,
        &catalog_bytes,
    )?;
    migrate_missing_managed_catalog_for_restore(
        codex_home,
        &backup_path,
        &mut backup_snapshot,
        &mut backup,
        &catalog_path,
        &catalog_bytes,
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
    let invalid_reason = if backup.version != 1 {
        Some("unsupported backup version")
    } else if backup.managed_key_hash.len() != 64 {
        Some("invalid managed key fingerprint")
    } else if backup.managed_base_url.trim().is_empty() {
        Some("missing managed gateway address")
    } else if !oauth_metadata_valid {
        Some("invalid OAuth ownership metadata")
    } else if !valid_managed_model_catalog(&backup, &catalog_path, &catalog_bytes)
        && !externally_changed_managed_model_catalog(&backup, &catalog_path, &catalog_bytes)
    {
        Some(
            if valid_managed_model_catalog_metadata(&backup, &catalog_path, &catalog_bytes) {
                "managed model catalog is missing or cannot be verified"
            } else {
                "invalid managed model catalog reference or fingerprint"
            },
        )
    } else {
        None
    };
    if let Some(reason) = invalid_reason {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("ChatGPT local gateway profile backup cannot be restored: {reason}"),
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
    catalog_bytes: &Option<Vec<u8>>,
) -> Result<()> {
    let legacy_metadata = backup.managed_model_catalog_path.is_none()
        && backup.managed_model_catalog_hash.is_none()
        && backup.managed_model_catalog_pending_hash.is_none()
        && !backup.managed_model_catalog_pending_remove;
    let Some(catalog_bytes) = catalog_bytes.as_deref() else {
        return Ok(());
    };
    if !legacy_metadata || !is_relay_managed_model_catalog(catalog_bytes) {
        return Ok(());
    }

    backup.managed_model_catalog_path = Some(portable_path_string(catalog_path));
    backup.managed_model_catalog_hash = Some(bytes_hash(catalog_bytes));
    if backup.previous_model_catalog_json.is_none() {
        let config_path = codex_home.join(CONFIG_FILE);
        let current_catalog = read_optional_bytes(&config_path)
            .ok()
            .flatten()
            .and_then(|config_bytes| {
                snapshot_text(&Some(config_bytes), &config_path)
                    .ok()
                    .flatten()
                    .map(str::to_owned)
            })
            .and_then(|config_text| parse_config(&config_text).ok())
            .and_then(|document| root_model_catalog_json(&document));
        if current_catalog
            .as_deref()
            .is_some_and(|configured_catalog_path| {
                !configured_catalog_matches_path(codex_home, configured_catalog_path, catalog_path)
            })
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
    let configured = PathBuf::from(portable_path_value(configured));
    let resolved = if configured.is_absolute() {
        configured
    } else {
        codex_home.join(configured)
    };
    portable_path_string(&resolved) == portable_path_string(expected)
}

/// Older Relay builds could leave the dedicated catalog file behind while the
/// managed profile still referenced it. The backup itself remains sufficient
/// to restore the profile, so persist a restore-pending marker rather than
/// trapping the user behind a metadata-only recovery error.
fn migrate_missing_managed_catalog_for_restore(
    codex_home: &Path,
    backup_path: &Path,
    backup_bytes: &mut Option<Vec<u8>>,
    backup: &mut ProfileBackup,
    catalog_path: &Path,
    catalog_bytes: &Option<Vec<u8>>,
) -> Result<()> {
    if catalog_bytes.is_some()
        || backup.restore_pending
        || backup.attach_pending
        || backup.managed_model_catalog_path.is_none()
        || backup.managed_model_catalog_hash.is_none()
    {
        return Ok(());
    }

    let config_path = codex_home.join(CONFIG_FILE);
    let config_bytes = read_optional_bytes(&config_path)?;
    let configured_catalog = snapshot_text(&config_bytes, &config_path)?
        .map(parse_config)
        .transpose()?
        .and_then(|document| root_model_catalog_json(&document));
    if !configured_catalog
        .as_deref()
        .is_some_and(|configured_catalog_path| {
            configured_catalog_matches_path(codex_home, configured_catalog_path, catalog_path)
        })
    {
        return Ok(());
    }

    backup.restore_pending = true;
    let updated = serialize_backup(backup)?;
    replace_if_unchanged(backup_path, backup_bytes, &updated)?;
    *backup_bytes = Some(updated.into_bytes());
    Ok(())
}

fn is_relay_managed_model_catalog(catalog_bytes: &[u8]) -> bool {
    catalog::read_catalog_values(catalog_bytes, true).is_ok_and(|models| {
        !models.is_empty()
            && models.iter().all(|model| {
                model.get("comp_hash").and_then(Value::as_str) == Some(CODEX_RELAY_CATALOG_HASH)
            })
    })
}

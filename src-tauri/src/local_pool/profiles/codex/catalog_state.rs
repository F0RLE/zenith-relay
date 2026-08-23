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
    if portable_path_value(path) != portable_path_string(expected_path) {
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

pub(super) fn managed_model_catalog_path(backup_root: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(backup_root).map_err(io_error)?;
    Ok(root.join(MODEL_CATALOG_FILE))
}

pub(super) fn apply_model_catalog_change(
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

pub(super) fn rollback_model_catalog_change(
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

pub(super) fn remove_managed_model_catalog_if_unchanged(backup: &ProfileBackup) {
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

pub(super) fn invalidate_models_cache(codex_home: &Path) -> Result<bool> {
    let path = codex_home.join(MODELS_CACHE_FILE);
    let snapshot = read_optional_bytes(&path)?;
    if snapshot.is_none() {
        return Ok(false);
    }
    remove_if_unchanged(&path, &snapshot)?;
    Ok(true)
}

pub(super) fn backup_path(root: &Path) -> PathBuf {
    root.join("codex-default.json")
}

pub(super) fn local_backup(codex_home: &Path, root: &Path) -> Result<Option<ProfileBackup>> {
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

    backup.managed_model_catalog_path = Some(portable_path_string(catalog_path));
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
    let configured = PathBuf::from(portable_path_value(configured));
    let resolved = if configured.is_absolute() {
        configured
    } else {
        codex_home.join(configured)
    };
    portable_path_string(&resolved) == portable_path_string(expected)
}

fn is_relay_managed_model_catalog(content: &[u8]) -> bool {
    catalog::read_catalog_values(content, true).is_ok_and(|models| {
        !models.is_empty()
            && models.iter().all(|model| {
                model.get("comp_hash").and_then(Value::as_str) == Some(CODEX_RELAY_CATALOG_HASH)
            })
    })
}

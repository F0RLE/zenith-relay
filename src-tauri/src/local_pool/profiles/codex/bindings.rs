use super::*;

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
    if let Some(backup_path) = account_backup_for_profile(codex_home, backup_root)? {
        let backup_bytes = read_optional_bytes(&backup_path)?;
        return Ok(parse_account_backup_snapshot(&backup_bytes, &backup_path)?
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
        let config_bytes = read_optional_bytes(&config_path)?;
        let document =
            parse_config(snapshot_text(&config_bytes, &config_path)?.unwrap_or_default())?;
        let auth_path = profile_dir.join(AUTH_FILE);
        let auth_bytes = read_optional_bytes(&auth_path)?;
        binding.active = account_managed_config_matches(&document)
            && account_auth_matches_snapshot(&auth_bytes, &auth_path, &backup.managed_access_hash)?;
    }
    if let Some(backup) = local_backup(codex_home, backup_root)? {
        let profile_dir = canonical_profile_dir(codex_home)?;
        let config_path = profile_dir.join(CONFIG_FILE);
        let config_bytes = read_optional_bytes(&config_path)?;
        let document =
            parse_config(snapshot_text(&config_bytes, &config_path)?.unwrap_or_default())?;
        let auth_path = profile_dir.join(AUTH_FILE);
        let auth_bytes = read_optional_bytes(&auth_path)?;
        let active = managed_config_matches(&document, &backup)
            && managed_auth_matches_snapshot(&auth_bytes, &auth_path, &backup)?;
        bindings.push(ProfileBinding {
            profile_dir: profile_dir.to_string_lossy().into_owned(),
            credential_kind: backup.credential_kind(),
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
        let config_bytes = read_optional_bytes(&config_path)?;
        let document =
            parse_config(snapshot_text(&config_bytes, &config_path)?.unwrap_or_default())?;
        if document_has_provider(&document) {
            bindings.push(ProfileBinding {
                profile_dir: profile_dir.to_string_lossy().into_owned(),
                credential_kind: ProfileCredentialKind::LocalGateway,
                credential_id: "local_gateway".to_string(),
                bound_oauth_account_id: None,
                active: false,
            });
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
    for directory_entry in fs::read_dir(backup_root).map_err(io_error)? {
        let directory_entry = directory_entry.map_err(io_error)?;
        let backup_file_name = directory_entry.file_name();
        let backup_file_name = backup_file_name.to_string_lossy();
        if !backup_file_name.starts_with(ACCOUNT_BACKUP_PREFIX)
            || !backup_file_name.ends_with(".json")
        {
            continue;
        }
        let backup_path = directory_entry.path();
        let backup_text =
            fs::read_to_string(&backup_path).map_err(|error| io_error_at(&backup_path, error))?;
        let backup = parse_account_backup(&backup_text, &backup_path)?;
        bindings.push(binding_from_backup(&backup, false));
    }
    bindings.sort_by(|left, right| left.profile_dir.cmp(&right.profile_dir));
    Ok(bindings)
}

mod tokens;
pub(crate) use tokens::managed_account_token_update;
pub(super) use tokens::managed_token;

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

    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let config_bytes = read_optional_bytes(&config_path)?;
    let auth_bytes = read_optional_bytes(&auth_path)?;
    let document = parse_config(snapshot_text(&config_bytes, &config_path)?.unwrap_or_default())?;
    let auth_matches_previous = managed_auth_matches_snapshot(&auth_bytes, &auth_path, &backup)?;
    let auth_matches_next =
        account_auth_matches_tokens(&auth_bytes, &auth_path, tokens, provider_account_id)?;
    if !managed_config_matches(&document, &backup) || (!auth_matches_previous && !auth_matches_next)
    {
        return Ok(false);
    }
    if auth_matches_next && backup.managed_oauth_access_hash.as_deref() == Some(next_hash.as_str())
    {
        return Ok(false);
    }

    backup.managed_oauth_access_hash = Some(next_hash);
    let updated_backup = serialize_backup(&backup)?;
    replace_if_unchanged(&backup_path, &backup_bytes, &updated_backup)?;
    if auth_matches_next {
        return Ok(true);
    }
    let credential = account_auth_content(tokens, provider_account_id)?;
    projection::update_auth_with_rollback(
        &auth_path,
        &auth_bytes,
        &credential,
        (&backup_path, &updated_backup, &backup_bytes),
    )
}

pub(crate) fn refresh_managed_model_catalog(
    codex_home: &Path,
    backup_root: &Path,
    catalog_json: &str,
    expected_binding: Option<&ProfileBinding>,
) -> Result<bool> {
    let _profile_guard = lock_codex_profile();
    let _ = local_backup(codex_home, backup_root)?;
    let backup_path = backup_path(backup_root);
    let mut backup_bytes = read_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(false);
    };
    if expected_binding.is_some_and(|binding| {
        binding.credential_kind != backup.credential_kind()
            || binding.credential_id != backup.managed_key_id
            || binding.bound_oauth_account_id != backup.bound_oauth_account_id
    }) {
        return Ok(false);
    }
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
    let config_bytes = read_optional_bytes(&config_path)?;
    let auth_bytes = read_optional_bytes(&auth_path)?;
    let document = parse_config(snapshot_text(&config_bytes, &config_path)?.unwrap_or_default())?;
    if !managed_config_matches(&document, &backup)
        || !managed_auth_matches_snapshot(&auth_bytes, &auth_path, &backup)?
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

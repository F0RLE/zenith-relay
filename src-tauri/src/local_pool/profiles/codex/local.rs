use super::*;
use std::{fs, path::Path};

pub(super) fn prepare_existing_local_binding_locked(
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

pub(super) fn attach_local_locked(
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
            catalog::build_managed_model_catalog(
                codex_home,
                user_catalog_path.as_deref(),
                had_managed_catalog
                    .then_some(original_catalog_bytes.as_deref())
                    .flatten(),
                content,
            )
        })
        .transpose()?;
    let managed_model_reasoning_effort = reasoning_effort_for_attach(&document, catalog.as_deref());

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
        managed_supports_websockets: true,
        managed_model_reasoning_effort_cleared: false,
        managed_model_reasoning_effort: None,
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
    backup.managed_model_reasoning_effort = managed_model_reasoning_effort.clone();
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
    backup.managed_supports_websockets = true;
    let previous_managed_catalog_path = backup.managed_model_catalog_path.clone();
    let previous_managed_catalog_hash = backup.managed_model_catalog_hash.clone();
    backup.managed_model_catalog_path = if catalog.is_some() {
        Some(portable_path_string(&catalog_path))
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
            .map(|_| portable_path_string(&catalog_path))
            .as_deref(),
        backup.previous_model_catalog_json.as_deref(),
        managed_model_reasoning_effort.as_deref(),
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
        .map(|_| portable_path_string(&catalog_path));
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

pub(super) fn restore_local_locked(
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
    validate_config_shape(&document)?;
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
    let current_model_reasoning_effort = root_model_reasoning_effort(&document);
    restore_local_config(
        &mut document,
        &backup,
        model_catalog.as_deref(),
        current_model_reasoning_effort.as_deref(),
    );
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

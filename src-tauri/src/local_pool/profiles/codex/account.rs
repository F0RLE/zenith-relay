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

    let orphaned_managed_provider = existing_backup.is_none() && document_has_provider(&document);
    if let Some(backup) = existing_backup.as_ref() {
        if backup.profile_dir != profile_dir.to_string_lossy()
            || !account_auth_matches_snapshot(
                &original_auth_bytes,
                &auth_path,
                &backup.managed_access_hash,
            )?
        {
            return Err(profile_restore_blocked());
        }
    }

    // Codex caches the model list independently from config.toml. Remove it
    // before changing credentials/config so failure to invalidate cannot
    // leave a newly attached account using the previous account's model list.
    invalidate_models_cache(&profile_dir)?;

    let created_backup = existing_backup.is_none();
    let mut backup = existing_backup.unwrap_or(AccountProfileBackup {
        version: 1,
        projection_secret_ref: None,
        profile_dir: profile_dir.to_string_lossy().into_owned(),
        previous_model_provider: (!orphaned_managed_provider)
            .then(|| root_model_provider(&document))
            .flatten(),
        previous_model_catalog_json: (!orphaned_managed_provider)
            .then(|| root_model_catalog_json(&document))
            .flatten(),
        previous_model: (!orphaned_managed_provider)
            .then(|| root_model(&document))
            .flatten(),
        previous_review_model: (!orphaned_managed_provider)
            .then(|| root_review_model(&document))
            .flatten(),
        previous_chatgpt_base_url: (!orphaned_managed_provider)
            .then(|| root_chatgpt_base_url(&document))
            .flatten(),
        previous_openai_base_url: (!orphaned_managed_provider)
            .then(|| root_openai_base_url(&document))
            .flatten(),
        previous_model_reasoning_effort: (!orphaned_managed_provider)
            .then(|| root_model_reasoning_effort(&document))
            .flatten(),
        previous_auth_secret_ref: None,
        managed_account_id: String::new(),
        managed_access_hash: String::new(),
    });
    // Older backups may not have the newer root fields. Read the encrypted
    // before-snapshot once and fill only missing fields from that source.
    fill_missing_account_config(&mut backup, &document, secrets)?;
    if created_backup {
        if let Some(previous_auth) = zenith_relay_core::omit_blank(original_auth) {
            let secret_ref = account_backup_secret_ref(&profile_dir);
            secrets.save(&secret_ref, previous_auth)?;
            backup.previous_auth_secret_ref = Some(secret_ref);
        }
    }
    backup.managed_account_id = account_id.to_string();
    backup.managed_access_hash = key_hash(tokens.access_token());
    attach_account_config(&mut document);
    let managed_config = document.to_string();
    let credential = account_auth_content(tokens, provider_account_id)?;
    let managed_auth = projection::merge_auth(original_auth, Some(&credential))?
        .expect("an attached credential is present");
    let old_projection_secret_ref = backup.projection_secret_ref.clone();
    let mut created_projection_secret = false;
    let projection_result = (|| -> Result<String> {
        match old_projection_secret_ref.as_deref() {
            Some(secret_ref) => {
                // Upgrade an older Relay projection in place semantically: keep
                // its original user config, but teach the undo record that these
                // catalog/provider leaves are cleared while ChatGPT OAuth is
                // active. Fork the secret so a failed file update can roll back.
                let projected_after = projection::config_after(secret_ref, secrets)?;
                let mut projected_document = parse_config(&projected_after)?;
                attach_account_config(&mut projected_document);
                let cleaned_projection = projected_document.to_string();
                if cleaned_projection == projected_after {
                    Ok(secret_ref.to_owned())
                } else {
                    let next_ref = projection::fork_with_config_after(
                        secret_ref,
                        &cleaned_projection,
                        secrets,
                    )?;
                    created_projection_secret = true;
                    Ok(next_ref)
                }
            }
            None => {
                let previous_auth = if created_backup {
                    original_auth.map(ToOwned::to_owned)
                } else if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
                    Some(secrets.load(secret_ref)?.ok_or_else(|| {
                        LocalPoolError::new(
                            ErrorCode::RecoveryRequired,
                            "ChatGPT account profile backup secret is missing",
                        )
                    })?)
                } else {
                    None
                };
                let next_ref = projection::save(
                    snapshot_text(&original_config_bytes, &config_path)?,
                    &managed_config,
                    previous_auth.as_deref(),
                    secrets,
                )?;
                created_projection_secret = true;
                Ok(next_ref)
            }
        }
    })();
    let projection_secret_ref = match projection_result {
        Ok(secret_ref) => secret_ref,
        Err(error) => {
            let cleanup = cleanup_account_attach_secrets(
                created_backup,
                created_projection_secret,
                &backup,
                secrets,
            );
            return Err(with_rollback(error, cleanup));
        }
    };
    backup.projection_secret_ref = Some(projection_secret_ref);
    let backup_content = match serialize_account_backup(&backup) {
        Ok(content) => content,
        Err(error) => {
            let cleanup = cleanup_account_attach_secrets(
                created_backup,
                created_projection_secret,
                &backup,
                secrets,
            );
            return Err(with_rollback(error, cleanup));
        }
    };
    if let Err(error) = replace_if_unchanged(&backup_path, &original_backup_bytes, &backup_content)
    {
        return Err(with_rollback(
            error,
            cleanup_account_attach_secrets(
                created_backup,
                created_projection_secret,
                &backup,
                secrets,
            ),
        ));
    }

    if let Err(error) = replace_if_unchanged(&config_path, &original_config_bytes, &managed_config)
    {
        return Err(with_rollback(
            error,
            rollback_account_backup(
                created_backup,
                created_projection_secret,
                &backup_path,
                &backup_content,
                &original_backup_bytes,
                &backup,
                secrets,
            ),
        ));
    }
    if let Err(error) = replace_if_unchanged(&auth_path, &original_auth_bytes, &managed_auth) {
        let config_rollback = rollback_file(&config_path, &managed_config, &original_config_bytes);
        let backup_rollback = rollback_account_backup(
            created_backup,
            created_projection_secret,
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
    if let Some(old_secret_ref) = old_projection_secret_ref.filter(|old_secret_ref| {
        Some(old_secret_ref.as_str()) != backup.projection_secret_ref.as_deref()
    }) {
        // A failed cleanup must not undo the committed profile switch. The
        // backup points at the new encrypted projection; an orphaned secret
        // can be cleaned up later without risking the active account.
        let _ = secrets.delete(&old_secret_ref);
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
    // A restored profile may use a different provider/catalog. Let Codex
    // rebuild the cache from that restored configuration on the next launch.
    invalidate_models_cache(&profile_dir)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let auth_path = profile_dir.join(AUTH_FILE);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let mut document = parse_config(original_config)?;
    if backup.projection_secret_ref.is_none() && !account_managed_config_matches(&document) {
        return Err(profile_restore_blocked());
    }
    let auth_matches_managed = account_auth_matches_snapshot(
        &original_auth_bytes,
        &auth_path,
        &backup.managed_access_hash,
    )?;
    let previous_auth = match (
        auth_matches_managed,
        backup.projection_secret_ref.as_deref(),
        backup.previous_auth_secret_ref.as_deref(),
    ) {
        (true, None, Some(secret_ref)) => Some(secrets.load(secret_ref)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT account profile backup secret is missing",
            )
        })?),
        _ => None,
    };
    restore_account_config(&mut document, &backup);
    let restored = projection::restore_from_backup(
        backup.projection_secret_ref.as_deref(),
        &document,
        (&config_path, &original_config_bytes),
        (&auth_path, &original_auth_bytes),
        auth_matches_managed,
        previous_auth.as_deref(),
        secrets,
    )?;
    if read_optional_bytes(&backup_path)? != backup_bytes {
        return Err(profile_changed_at(&backup_path));
    }
    let restored_config_bytes = restored
        .config
        .as_ref()
        .map(|text| text.as_bytes().to_vec());
    replace_with_snapshot(
        &config_path,
        &original_config_bytes,
        restored.config.as_deref(),
    )?;

    let restored_auth_bytes = if auth_matches_managed {
        restored
            .auth
            .as_ref()
            .map(|content| content.as_bytes().to_vec())
    } else {
        original_auth_bytes.clone()
    };
    let auth_result = match (auth_matches_managed, restored.auth.as_deref()) {
        (true, Some(previous_auth)) => {
            replace_if_unchanged(&auth_path, &original_auth_bytes, previous_auth)
        }
        (true, None) => remove_if_unchanged(&auth_path, &original_auth_bytes),
        (false, _) => Ok(()),
    };
    if let Err(error) = auth_result {
        return Err(with_rollback(
            error,
            restore_snapshot_if_unchanged(
                &config_path,
                &restored_config_bytes,
                &original_config_bytes,
            ),
        ));
    }
    if let Err(error) = remove_if_unchanged(&backup_path, &backup_bytes) {
        let auth_rollback =
            restore_snapshot_if_unchanged(&auth_path, &restored_auth_bytes, &original_auth_bytes);
        let config_rollback = restore_snapshot_if_unchanged(
            &config_path,
            &restored_config_bytes,
            &original_config_bytes,
        );
        return Err(with_rollback(
            error,
            merge_rollbacks(auth_rollback, config_rollback),
        ));
    }
    if let Err(error) = delete_backup_secrets(
        backup.previous_auth_secret_ref.as_deref(),
        backup.projection_secret_ref.as_deref(),
        secrets,
    ) {
        let backup_rollback = restore_snapshot_if_unchanged(&backup_path, &None, &backup_bytes);
        let auth_rollback =
            restore_snapshot_if_unchanged(&auth_path, &restored_auth_bytes, &original_auth_bytes);
        let config_rollback = restore_snapshot_if_unchanged(
            &config_path,
            &restored_config_bytes,
            &original_config_bytes,
        );
        return Err(with_rollback(
            error,
            merge_rollbacks(
                backup_rollback,
                merge_rollbacks(auth_rollback, config_rollback),
            ),
        ));
    }
    Ok(Some(binding_from_backup(&backup, auth_matches_managed)))
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
    let auth_path = profile_dir.join(AUTH_FILE);
    let auth = read_optional_bytes(&auth_path)?;
    let auth_matches_previous =
        account_auth_matches_snapshot(&auth, &auth_path, &backup.managed_access_hash)?;
    let auth_matches_next =
        account_auth_matches_tokens(&auth, &auth_path, tokens, provider_account_id)?;
    if !auth_matches_previous && !auth_matches_next {
        return Ok(false);
    }
    if auth_matches_next && backup.managed_access_hash == next_hash {
        return Ok(false);
    }
    // A refreshed OAuth token may carry a changed subscription/entitlement.
    // Codex caches model availability separately from auth.json.
    invalidate_models_cache(&profile_dir)?;
    backup.managed_access_hash = next_hash;
    let updated_backup = serialize_account_backup(&backup)?;
    replace_if_unchanged(&backup_path, &backup_bytes, &updated_backup)?;
    if auth_matches_next {
        return Ok(true);
    }
    let credential = account_auth_content(tokens, provider_account_id)?;
    projection::update_auth_with_rollback(
        &auth_path,
        &auth,
        &credential,
        (&backup_path, &updated_backup, &backup_bytes),
    )
}

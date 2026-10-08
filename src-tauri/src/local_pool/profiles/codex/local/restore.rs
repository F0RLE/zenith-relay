use super::*;
use std::{thread, time::Duration};

pub(in crate::local_pool::profiles::codex) fn restore_local_locked(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _ = local_backup(codex_home, backup_root)?;
    let backup_path = backup_path(backup_root);
    // Codex can refresh auth/config immediately after its process is stopped.
    // Reading one file while that refresh is in progress used to make the
    // restore path report a permanent profile conflict. Require a short,
    // stable snapshot before comparing the managed profile; a real external
    // takeover still fails closed below.
    let mut backup_bytes = read_stable_optional_bytes(&backup_path)?;
    let Some(mut backup) = parse_backup_snapshot(&backup_bytes, &backup_path)? else {
        return Ok(());
    };
    let catalog_path = managed_model_catalog_path(backup_root)?;
    let catalog_bytes = read_stable_optional_bytes(&catalog_path)?;
    let external_catalog =
        externally_changed_managed_model_catalog(&backup, &catalog_path, &catalog_bytes);
    if !valid_managed_model_catalog(&backup, &catalog_path, &catalog_bytes) && !external_catalog {
        return Err(profile_restore_blocked());
    }
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let mut original_config_bytes = read_stable_optional_bytes(&config_path)?;
    let original_auth_bytes = read_stable_optional_bytes(&auth_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let mut document = parse_config(original_config)?;
    validate_config_shape(&document)?;
    let config_matches_managed = managed_config_matches(&document, &backup);
    let config_matches_previous = previous_config_matches(&document, &backup);
    if backup.projection_secret_ref.is_none() && !config_matches_managed && !config_matches_previous
    {
        return Err(profile_restore_blocked());
    }
    if config_matches_managed && normalize_managed_provider_name(&mut document, &backup) {
        let normalized = document.to_string();
        replace_if_unchanged(&config_path, &original_config_bytes, &normalized)?;
        original_config_bytes = Some(normalized.into_bytes());
    }
    let auth_matches_managed =
        managed_auth_matches_snapshot(&original_auth_bytes, &auth_path, &backup)?;
    let previous_auth = if auth_matches_managed && backup.projection_secret_ref.is_none() {
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
        if backup.previous_auth_secret_ref.is_some() && previous_auth.is_none() {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT profile backup secret is missing",
            ));
        }
        previous_auth
    } else {
        None
    };
    if read_stable_optional_bytes(&backup_path)? != backup_bytes {
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
    let restored = projection::restore_from_backup(
        backup.projection_secret_ref.as_deref(),
        &document,
        (&config_path, &original_config_bytes),
        (&auth_path, &original_auth_bytes),
        auth_matches_managed,
        previous_auth.as_deref(),
        secrets,
    )?;
    if read_stable_optional_bytes(&backup_path)? != backup_bytes {
        return Err(profile_changed_at(&backup_path));
    }
    replace_with_snapshot(
        &config_path,
        &original_config_bytes,
        restored.config.as_deref(),
    )?;

    if auth_matches_managed {
        let auth_result = match restored.auth.as_deref() {
            Some(previous_auth) => {
                replace_if_unchanged(&auth_path, &original_auth_bytes, previous_auth)
            }
            None => remove_if_unchanged(&auth_path, &original_auth_bytes),
        };
        if let Err(error) = auth_result {
            let restored_config_bytes = restored
                .config
                .as_ref()
                .map(|text| text.as_bytes().to_vec());
            return Err(with_rollback(
                error,
                restore_snapshot_if_unchanged(
                    &config_path,
                    &restored_config_bytes,
                    &original_config_bytes,
                ),
            ));
        }
    }

    if catalog_bytes.is_some() && !external_catalog {
        remove_if_unchanged(&catalog_path, &catalog_bytes)?;
    }
    discard_backup(
        &backup_path,
        &backup_bytes,
        backup.previous_auth_secret_ref.as_deref(),
        backup.projection_secret_ref.as_deref(),
        secrets,
    )?;
    if backup.managed_model_catalog_path.is_some() {
        let _ = invalidate_models_cache(codex_home);
    }
    Ok(())
}

/// Read a profile file only after two consecutive reads agree. Codex writes
/// auth/config atomically but may perform a second refresh immediately after
/// the first rename. Treat that short write window as transient rather than
/// turning it into `profile_restore_blocked`.
fn read_stable_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut previous_bytes = read_optional_bytes(path)?;
    for _ in 0..3 {
        thread::sleep(Duration::from_millis(20));
        let candidate_bytes = read_optional_bytes(path)?;
        if candidate_bytes == previous_bytes {
            return Ok(candidate_bytes);
        }
        previous_bytes = candidate_bytes;
    }
    Ok(previous_bytes)
}

use super::super::*;

pub(in crate::local_pool::profiles::codex) fn parse_backup_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
) -> Result<Option<ProfileBackup>> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(None);
    };
    serde_json::from_str(content).map(Some).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("ChatGPT profile backup is invalid: {error}"),
        )
    })
}

pub(in crate::local_pool::profiles::codex) fn serialize_backup(
    backup: &ProfileBackup,
) -> Result<String> {
    let content = serde_json::to_string_pretty(backup).map_err(LocalPoolError::invalid_state)?;
    Ok(format!("{content}\n"))
}

pub(in crate::local_pool::profiles::codex) fn rollback_backup(
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

pub(in crate::local_pool::profiles::codex) fn cleanup_created_backup_secret(
    created: bool,
    backup: &ProfileBackup,
    secrets: &impl SecretBackend,
) -> Result<()> {
    if !created {
        return Ok(());
    }
    delete_backup_secrets(
        backup.previous_auth_secret_ref.as_deref(),
        backup.projection_secret_ref.as_deref(),
        secrets,
    )
}

pub(in crate::local_pool::profiles::codex) fn restore_secret_snapshot(
    snapshot: &Option<(String, Option<String>)>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let Some((secret_ref, value)) = snapshot else {
        return Ok(());
    };
    match value {
        Some(value) => secrets.save(secret_ref, value),
        None => secrets.delete(secret_ref),
    }
}

pub(in crate::local_pool::profiles::codex) fn previous_auth_snapshot(
    secret_ref: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<Option<String>> {
    secret_ref
        .map(|secret_ref| {
            secrets.load(secret_ref)?.ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "ChatGPT profile backup secret is missing",
                )
            })
        })
        .transpose()
}

pub(in crate::local_pool::profiles::codex) fn discard_managed_binding_locked(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<()> {
    if let Some(path) = super::account_backup_for_profile(codex_home, backup_root)? {
        let bytes = read_optional_bytes(&path)?;
        let backup = super::parse_account_backup_snapshot(&bytes, &path)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT account profile backup disappeared during snapshot restore",
            )
        })?;
        return discard_backup(
            &path,
            &bytes,
            backup.previous_auth_secret_ref.as_deref(),
            backup.projection_secret_ref.as_deref(),
            secrets,
        );
    }
    let path = backup_path(backup_root);
    let bytes = read_optional_bytes(&path)?;
    let Some(backup) = parse_backup_snapshot(&bytes, &path)? else {
        return Ok(());
    };
    discard_backup(
        &path,
        &bytes,
        backup.previous_auth_secret_ref.as_deref(),
        backup.projection_secret_ref.as_deref(),
        secrets,
    )?;
    remove_managed_model_catalog_if_unchanged(&backup);
    Ok(())
}

pub(in crate::local_pool::profiles::codex) fn discard_backup(
    path: &Path,
    bytes: &Option<Vec<u8>>,
    previous_auth_secret_ref: Option<&str>,
    projection_secret_ref: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    remove_if_unchanged(path, bytes)?;
    if let Err(error) =
        delete_backup_secrets(previous_auth_secret_ref, projection_secret_ref, secrets)
    {
        return Err(with_rollback(
            error,
            restore_snapshot_if_unchanged(path, &None, bytes),
        ));
    }
    Ok(())
}

/// Keep a retryable pair when deletion of either encrypted payload fails.
pub(in crate::local_pool::profiles::codex) fn delete_backup_secrets(
    previous_auth_secret_ref: Option<&str>,
    projection_secret_ref: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let snapshots = [previous_auth_secret_ref, projection_secret_ref]
        .into_iter()
        .flatten()
        .map(|secret_ref| secrets.load(secret_ref).map(|value| (secret_ref, value)))
        .collect::<Result<Vec<_>>>()?;
    for (index, (secret_ref, _)) in snapshots.iter().enumerate() {
        if let Err(error) = secrets.delete(secret_ref) {
            let mut rollback = Ok(());
            for (deleted_ref, value) in &snapshots[..index] {
                if let Some(value) = value {
                    rollback = merge_rollbacks(rollback, secrets.save(deleted_ref, value));
                }
            }
            return Err(with_rollback(error, rollback));
        }
    }
    Ok(())
}

use super::*;
use std::path::Path;

mod attach;
mod restore;
pub(super) use attach::attach_local_locked;
pub(super) use restore::restore_local_locked;

pub(super) fn prepare_existing_local_binding_locked(
    codex_home: &Path,
    backup_root: &Path,
    rebase_newer_login: bool,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _ = local_backup(codex_home, backup_root)?;
    let backup_file_path = backup_path(backup_root);
    let backup_bytes = read_optional_bytes(&backup_file_path)?;
    let Some(mut backup) = parse_backup_snapshot(&backup_bytes, &backup_file_path)? else {
        return Ok(());
    };
    let profile_dir = canonical_profile_dir(codex_home)?;
    let config_path = profile_dir.join(CONFIG_FILE);
    let config_bytes = read_optional_bytes(&config_path)?;
    let mut config_document =
        parse_config(snapshot_text(&config_bytes, &config_path)?.unwrap_or_default())?;
    if external_provider_took_over(&config_document, &backup) {
        return Ok(());
    }
    // Only an explicit activation may adopt a newer user login as the next
    // baseline. Automatic rollbacks still refuse to replace that login.
    if !rebase_newer_login {
        ensure_no_newer_login(&profile_dir, &backup)?;
    }
    if managed_config_matches(&config_document, &backup)
        && normalize_managed_provider_name(&mut config_document, &backup)
    {
        replace_if_unchanged(&config_path, &config_bytes, &config_document.to_string())?;
    }
    if backup.previous_model_catalog_json.is_none()
        && backup.managed_model_catalog_path.is_none()
        && backup.managed_model_catalog_hash.is_none()
        && backup.managed_model_catalog_pending_hash.is_none()
        && managed_config_matches(&config_document, &backup)
    {
        if let Some(legacy_catalog) = root_model_catalog_json(&config_document) {
            backup.previous_model_catalog_json = Some(legacy_catalog);
            let updated = serialize_backup(&backup)?;
            replace_if_unchanged(&backup_file_path, &backup_bytes, &updated)?;
        }
    }
    restore_local_locked(codex_home, backup_root, secrets)
}

/// Explicit recovery can leave a new login alone. A switch that will write a
/// replacement credential must instead stop before touching that login.
pub(super) fn ensure_no_newer_login(profile_dir: &Path, backup: &ProfileBackup) -> Result<()> {
    let auth_path = profile_dir.join(AUTH_FILE);
    let auth_bytes = read_optional_bytes(&auth_path)?;
    if !managed_auth_matches_snapshot(&auth_bytes, &auth_path, backup)?
        && !previous_auth_matches_snapshot(&auth_bytes, backup)
    {
        return Err(profile_restore_blocked());
    }
    Ok(())
}

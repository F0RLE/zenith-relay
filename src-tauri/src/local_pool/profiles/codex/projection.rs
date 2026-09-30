//! The encrypted undo record contains exact before/after documents, not a
//! guessed provider from a rotating diagnostic backup. Only changed TOML
//! leaves and the authentication credential block belong to the attachment.
use super::*;

pub(super) fn update_auth_with_rollback(
    auth_path: &Path,
    auth: &Option<Vec<u8>>,
    credential: &str,
    backup: (&Path, &str, &Option<Vec<u8>>),
) -> Result<bool> {
    let update = (|| {
        let updated = merge_auth(snapshot_text(auth, auth_path)?, Some(credential))?
            .ok_or_else(|| LocalPoolError::invalid_state("updated credential is missing"))?;
        replace_if_unchanged(auth_path, auth, &updated)
    })();
    update
        .map(|()| true)
        .map_err(|error| with_rollback(error, rollback_file(backup.0, backup.1, backup.2)))
}

#[derive(Serialize, Deserialize)]
struct Projection {
    version: u32,
    config_before: Option<String>,
    config_after: String,
    auth_before: Option<String>,
}

pub(super) fn save(
    config_before: Option<&str>,
    config_after: &str,
    auth_before: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<String> {
    let projection = Projection {
        version: 1,
        config_before: config_before.map(str::to_owned),
        config_after: config_after.to_owned(),
        auth_before: auth_before.map(str::to_owned),
    };
    let secret_ref = format!("profile:codex:projection:{}", uuid::Uuid::new_v4());
    secrets.save(
        &secret_ref,
        &serde_json::to_string(&projection).map_err(LocalPoolError::invalid_state)?,
    )?;
    Ok(secret_ref)
}

fn load(secret_ref: &str, secrets: &impl SecretBackend) -> Result<Projection> {
    let content = secrets.load(secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Profile undo record is missing",
        )
    })?;
    let projection: Projection = serde_json::from_str(&content).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Profile undo record is invalid",
        )
    })?;
    if projection.version != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Unsupported profile undo record version",
        ));
    }
    Ok(projection)
}

pub(super) fn update_websockets(
    secret_ref: &str,
    provider_id: &str,
    enabled: bool,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let mut projection = load(secret_ref, secrets)?;
    let mut after = parse_config(&projection.config_after)?;
    if !set_managed_websockets(&mut after, provider_id, enabled) {
        return Err(profile_restore_blocked());
    }
    // Update only our setting: current user-added fields must never become
    // part of the managed projection and disappear on restore.
    projection.config_after = after.to_string();
    let content = serde_json::to_string(&projection).map_err(LocalPoolError::invalid_state)?;
    secrets.save(secret_ref, &content)
}

/// Record the Ultra picker switch on an existing undo snapshot. A later
/// attach must not leave the switch behind when the original profile is restored.
pub(super) fn record_show_ultra_picker(
    secret_ref: &str,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let mut projection = load(secret_ref, secrets)?;
    let mut after = parse_config(&projection.config_after)?;
    if desktop_bool(&after, DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY) == Some(true) {
        return Ok(());
    }
    enable_show_ultra_picker(&mut after);
    if desktop_bool(&after, DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY) != Some(true) {
        return Ok(());
    }
    projection.config_after = after.to_string();
    let content = serde_json::to_string(&projection).map_err(LocalPoolError::invalid_state)?;
    secrets.save(secret_ref, &content)
}

mod auth;
mod restore;

pub(super) use auth::merge_auth;
pub(super) use restore::{restore, restore_from_backup};

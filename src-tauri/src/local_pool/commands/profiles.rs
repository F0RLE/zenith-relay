use super::runtime_from_store;
use crate::{
    local_pool::{
        commands::accounts::prepare_account_credentials,
        error::{CommandError, ErrorCode, LocalPoolError},
        profiles::{codex, opencode, repair},
        state::DesktopState,
        store::secret_store,
    },
    platform::{default_codex_home, default_opencode_auth, default_opencode_config},
};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

#[tauri::command]
pub async fn attach_codex_to_local_gateway(
    key_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let (key, port) = {
        let store = state.store()?;
        let key = store
            .key(&key_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
        (key, store.gateway().port)
    };
    if !key.enabled || !super::pool::has_usable_source(&state, &key)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "local key is not available for any enabled candidate",
        )
        .into());
    }
    let secret = secret_store::load(&key.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key secret is missing"))?;
    codex::attach(
        &default_codex_home(),
        &state.profile_backup_root(),
        &format!("http://127.0.0.1:{port}/v1"),
        &secret,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub async fn restore_codex_profile(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    codex::restore(&default_codex_home(), &state.profile_backup_root()).map_err(Into::into)
}

#[tauri::command]
pub async fn attach_opencode_to_local_gateway(
    key_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let (key, port) = {
        let store = state.store()?;
        let key = store
            .key(&key_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
        (key, store.gateway().port)
    };
    if !key.enabled || !super::pool::has_usable_source(&state, &key)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "local key is not available for any enabled candidate",
        )
        .into());
    }
    let secret = secret_store::load(&key.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key secret is missing"))?;
    let models = runtime_from_store(&state).await?.visible_models_for_secret(
        &secret,
        &[
            zenith_relay_core::WireApi::Responses,
            zenith_relay_core::WireApi::ChatCompletions,
        ],
        now_ms(),
    );
    opencode::attach(
        &default_opencode_config(),
        &default_opencode_auth(),
        &state.profile_backup_root(),
        &format!("http://127.0.0.1:{port}/v1"),
        &secret,
        &models,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub async fn restore_opencode_profile(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    opencode::restore(
        &default_opencode_config(),
        &default_opencode_auth(),
        &state.profile_backup_root(),
    )
    .map_err(Into::into)
}

#[tauri::command]
pub fn get_opencode_profile_state(
    state: State<'_, DesktopState>,
) -> Result<opencode::ProfileState, CommandError> {
    opencode::state(
        &default_opencode_config(),
        &default_opencode_auth(),
        &state.profile_backup_root(),
    )
    .map_err(Into::into)
}

#[tauri::command]
pub async fn attach_codex_to_account(
    account_id: String,
    profile_dir: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<codex::ProfileBinding, CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = resolve_profile_dir(profile_dir)?;
    let prepared = prepare_account_credentials(&state, &account_id).await?;
    codex::attach_account(
        &profile_dir,
        &state.profile_backup_root(),
        &account_id,
        prepared.tokens(),
        prepared.provider_account_id(),
    )
    .map_err(Into::into)
}

#[tauri::command]
pub fn list_codex_account_bindings(
    state: State<'_, DesktopState>,
) -> Result<Vec<codex::ProfileBinding>, CommandError> {
    codex::account_bindings(&state.profile_backup_root()).map_err(Into::into)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepairPreviewInput {
    profile_dirs: Vec<String>,
    target_provider: repair::TargetProvider,
}

#[tauri::command]
pub fn preview_codex_history_repair(
    input: RepairPreviewInput,
    state: State<'_, DesktopState>,
) -> Result<repair::RepairPreview, CommandError> {
    let profiles = if input.profile_dirs.is_empty() {
        vec![resolve_profile_dir(None)?]
    } else {
        input
            .profile_dirs
            .into_iter()
            .map(|path| resolve_profile_dir(Some(path)))
            .collect::<Result<Vec<_>, _>>()?
    };
    repair::preview(
        state.root(),
        &profiles,
        input.target_provider,
        crate::launcher::is_codex_running(),
    )
    .map_err(repair_error)
}

#[tauri::command]
pub async fn apply_codex_history_repair(
    session_id: String,
    state: State<'_, DesktopState>,
) -> Result<repair::RepairResult, CommandError> {
    let _mutation = state.setup_guard().await;
    ensure_codex_stopped()?;
    repair::apply(state.root(), &state.profile_backup_root(), &session_id).map_err(repair_error)
}

#[tauri::command]
pub async fn rollback_codex_history_repair(
    backup_id: String,
    state: State<'_, DesktopState>,
) -> Result<repair::RollbackResult, CommandError> {
    let _mutation = state.setup_guard().await;
    ensure_codex_stopped()?;
    repair::rollback(&state.profile_backup_root(), &backup_id).map_err(repair_error)
}

#[tauri::command]
pub async fn restore_codex_account_profile(
    profile_dir: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<Option<codex::ProfileBinding>, CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = resolve_profile_dir(profile_dir)?;
    codex::restore_account_profile(&profile_dir, &state.profile_backup_root()).map_err(Into::into)
}

fn resolve_profile_dir(profile_dir: Option<String>) -> Result<PathBuf, CommandError> {
    let Some(profile_dir) = profile_dir else {
        return Ok(default_codex_home());
    };
    let profile_dir = profile_dir.trim();
    if profile_dir.is_empty() || profile_dir.chars().any(char::is_control) {
        return Err(LocalPoolError::new(ErrorCode::InvalidState, "profile path is invalid").into());
    }
    let path = PathBuf::from(profile_dir);
    if !path.is_absolute() {
        return Err(
            LocalPoolError::new(ErrorCode::InvalidState, "profile path must be absolute").into(),
        );
    }
    let canonical = std::fs::canonicalize(&path).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to access profile path: {error}"),
        )
    })?;
    if !canonical.is_dir() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "profile path is not a directory",
        )
        .into());
    }
    Ok(canonical)
}

fn repair_error(error: String) -> CommandError {
    LocalPoolError::new(ErrorCode::RecoveryRequired, error).into()
}

fn ensure_codex_stopped() -> Result<(), CommandError> {
    if crate::launcher::is_codex_running() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "close all Codex instances before changing history",
        )
        .into());
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

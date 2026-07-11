use super::runtime_from_store;
use crate::{
    local_pool::{
        commands::accounts::prepare_account_credentials,
        error::{CommandError, ErrorCode, LocalPoolError},
        profiles::{codex, opencode},
        state::DesktopState,
        store::secret_store,
    },
    platform::{default_codex_home, default_opencode_auth, default_opencode_config},
};
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

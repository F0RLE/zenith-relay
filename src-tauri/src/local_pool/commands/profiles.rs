use super::runtime_from_store;
use crate::{
    launcher::{launch_codex_with_profile, stop_codex_and_wait},
    local_pool::{
        commands::accounts::{prepare_account_credentials, sync_managed_account_profile},
        error::{CommandError, ErrorCode, LocalPoolError},
        profiles::{codex, opencode, repair},
        state::DesktopState,
        store::secret_store,
    },
    platform::{default_codex_home, default_opencode_auth, default_opencode_config},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileActivation {
    binding: codex::ProfileBinding,
    previous_credential_kind: Option<codex::ProfileCredentialKind>,
    repair_recommended: bool,
    stopped_running_client: bool,
}

#[tauri::command]
pub async fn attach_codex_to_local_gateway(
    key_id: String,
    bound_oauth_account_id: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
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
    let profile_dir = default_codex_home();
    let previous = codex::credential_kind(&profile_dir, &state.profile_backup_root())?;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let bound_oauth = match bound_oauth_account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(account_id) => Some((
            account_id.to_string(),
            prepare_account_credentials(&state, account_id).await?,
        )),
        None => None,
    };
    let base_url = format!("http://127.0.0.1:{port}/v1");
    let binding = match bound_oauth.as_ref() {
        Some((account_id, prepared)) => codex::attach_with_oauth(
            &profile_dir,
            &state.profile_backup_root(),
            &key_id,
            &base_url,
            &secret,
            codex::BoundOAuthProfile {
                account_id,
                tokens: prepared.tokens(),
                provider_account_id: prepared.provider_account_id(),
            },
        )?,
        None => codex::attach(
            &profile_dir,
            &state.profile_backup_root(),
            &key_id,
            &base_url,
            &secret,
        )?,
    };
    Ok(profile_activation(binding, previous, stopped))
}

#[tauri::command]
pub async fn restore_codex_profile(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    stop_codex_and_sync_account(&state).await?;
    codex::restore(&default_codex_home(), &state.profile_backup_root()).map_err(Into::into)
}

#[tauri::command]
pub async fn stop_managed_codex_profile(
    state: State<'_, DesktopState>,
) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    stop_codex_and_sync_account(&state).await
}

#[tauri::command]
pub async fn launch_managed_codex_profile(
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    launch_codex_with_profile().map_err(|error| {
        LocalPoolError::new(ErrorCode::Io, format!("failed to launch Codex: {error}")).into()
    })
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
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    activate_account_profile(&account_id, profile_dir, &state).await
}

#[tauri::command]
pub async fn launch_codex_account(
    account_id: String,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    activate_account_profile(&account_id, None, &state).await
}

#[tauri::command]
pub fn list_codex_account_bindings(
    state: State<'_, DesktopState>,
) -> Result<Vec<codex::ProfileBinding>, CommandError> {
    codex::profile_bindings(&default_codex_home(), &state.profile_backup_root()).map_err(Into::into)
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
    stop_codex_and_sync_account_at(&state, &profile_dir).await?;
    codex::restore_account_profile(&profile_dir, &state.profile_backup_root()).map_err(Into::into)
}

async fn activate_account_profile(
    account_id: &str,
    profile_dir: Option<String>,
    state: &DesktopState,
) -> Result<ProfileActivation, CommandError> {
    let profile_dir = resolve_profile_dir(profile_dir)?;
    let previous = codex::credential_kind(&profile_dir, &state.profile_backup_root())?;
    let stopped = stop_codex_and_sync_account_at(state, &profile_dir).await?;
    let prepared = prepare_account_credentials(state, account_id).await?;
    let binding = codex::attach_account(
        &profile_dir,
        &state.profile_backup_root(),
        account_id,
        prepared.tokens(),
        prepared.provider_account_id(),
    )?;
    Ok(profile_activation(binding, previous, stopped))
}

fn profile_activation(
    binding: codex::ProfileBinding,
    previous: Option<codex::ProfileCredentialKind>,
    stopped_running_client: bool,
) -> ProfileActivation {
    ProfileActivation {
        repair_recommended: previous.is_some_and(|kind| kind != binding.credential_kind),
        binding,
        previous_credential_kind: previous,
        stopped_running_client,
    }
}

fn stop_codex_for_profile_change() -> Result<bool, CommandError> {
    stop_codex_and_wait().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to stop Codex before changing its profile: {error}"),
        )
        .into()
    })
}

async fn stop_codex_and_sync_account(state: &DesktopState) -> Result<bool, CommandError> {
    stop_codex_and_sync_account_at(state, &default_codex_home()).await
}

async fn stop_codex_and_sync_account_at(
    state: &DesktopState,
    profile_dir: &std::path::Path,
) -> Result<bool, CommandError> {
    let stopped = stop_codex_for_profile_change()?;
    if let Some(account_id) =
        codex::active_managed_account_id(profile_dir, &state.profile_backup_root())?
    {
        sync_managed_account_profile(state, &account_id).await?;
    }
    Ok(stopped)
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

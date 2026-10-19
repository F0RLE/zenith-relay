use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    state::DesktopState,
    store::secret_store,
};
use serde::Serialize;
use tauri::State;

mod library;
mod models;
mod paths;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeDesktopStatus {
    configured: bool,
    model_count: usize,
    has_recovery: bool,
    snapshot_name: Option<String>,
    snapshot_created_at_ms: Option<u64>,
    running: bool,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeDesktopConnection {
    model_count: usize,
    backup_created: bool,
}

#[tauri::command]
pub async fn get_claude_desktop_status(
    state: State<'_, DesktopState>,
) -> Result<ClaudeDesktopStatus, CommandError> {
    let _mutation = state.setup_guard().await;
    let path = paths::config_library()?;
    let status = library::inspect(&path, &state.claude_desktop_recovery_file())?;
    Ok(ClaudeDesktopStatus {
        configured: status.configured,
        model_count: status.model_count,
        has_recovery: status.has_recovery,
        snapshot_name: status.snapshot_name,
        snapshot_created_at_ms: status.created_at_ms,
        running: crate::launcher::is_claude_desktop_running(),
        path: path.display().to_string(),
    })
}

#[tauri::command]
pub async fn create_claude_desktop_snapshot(
    state: State<'_, DesktopState>,
    name: String,
) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    library::create_snapshot(
        &paths::config_library()?,
        &state.claude_desktop_recovery_file(),
        &name,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub async fn connect_claude_desktop(
    state: State<'_, DesktopState>,
) -> Result<ClaudeDesktopConnection, CommandError> {
    let _mutation = state.setup_guard().await;
    let address = state.gateway.address().await.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "Start the local pool before connecting Claude Desktop",
        )
    })?;
    let snapshot = super::state::build_local_runtime_state(&state).await?;
    let models = models::inference_models(&snapshot.gateway.models)?;
    if models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "The pool has no enabled models available to Claude Desktop; add and enable at least one model with a compatible route",
        )
        .into());
    }
    let key = super::pool::ensure_system_gateway_key(&state)?;
    if !key.enabled || !super::pool::has_usable_pool_candidate(&state)? {
        return Err(
            LocalPoolError::new(ErrorCode::Conflict, "The local pool is unavailable").into(),
        );
    }
    let secret = super::pool::ensure_local_gateway_key_secret(&key)?;
    let model_count = models.len();
    let config = models::gateway_config(&format!("http://{address}"), &secret, models);
    let path = paths::config_library()?;
    let recovery = state.claude_desktop_recovery_file();
    // Validate before closing the client. apply repeats these ownership checks
    // after shutdown, so changes during shutdown are not overwritten.
    library::check_attach(&path, &recovery, &config)?;
    stop_desktop().await?;
    let backup_created = library::apply(&path, &recovery, &config, true)?;
    Ok(ClaudeDesktopConnection {
        model_count,
        backup_created,
    })
}

#[tauri::command]
pub async fn launch_claude_desktop(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    // This command can be invoked independently of connect.
    let status = library::inspect(
        &paths::config_library()?,
        &state.claude_desktop_recovery_file(),
    )?;
    if !status.configured {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Connect Claude Desktop to the local pool before launching",
        )
        .into());
    }
    if state.gateway.address().await.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "Start the local pool before launching Claude Desktop",
        )
        .into());
    }
    if !super::pool::has_usable_pool_candidate(&state)? {
        return Err(
            LocalPoolError::new(ErrorCode::Conflict, "The local pool is unavailable").into(),
        );
    }
    refresh_catalog(&state).await?;
    tauri::async_runtime::spawn_blocking(|| crate::launcher::launch_claude_desktop(true))
        .await
        .map_err(|_| LocalPoolError::new(ErrorCode::Io, "Claude Desktop launch was interrupted"))?
        .map_err(|message| LocalPoolError::new(ErrorCode::Io, message).into())
}

#[tauri::command]
pub async fn restore_claude_desktop(state: State<'_, DesktopState>) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    if library::check_restore(
        &paths::config_library()?,
        &state.claude_desktop_recovery_file(),
    )? {
        stop_desktop().await?;
    }
    restore_connection(&state).map_err(Into::into)
}

async fn stop_desktop() -> LocalResult<()> {
    tauri::async_runtime::spawn_blocking(crate::launcher::stop_claude_desktop_and_wait)
        .await
        .map_err(|_| LocalPoolError::new(ErrorCode::Io, "Claude Desktop shutdown was interrupted"))?
        .map(|_| ())
        .map_err(|message| LocalPoolError::new(ErrorCode::Io, message))
}

pub(in crate::local_pool) fn restore_connection(state: &DesktopState) -> LocalResult<bool> {
    let recovery = state.claude_desktop_recovery_file();
    if !recovery.exists() {
        return Ok(false);
    }
    library::restore(&paths::config_library()?, &recovery)
}

pub(in crate::local_pool) async fn refresh_catalog(state: &DesktopState) -> LocalResult<()> {
    let recovery = state.claude_desktop_recovery_file();
    if !recovery.exists() {
        return Ok(());
    }
    let path = paths::config_library()?;
    if !library::inspect(&path, &recovery)?.configured {
        return Ok(());
    }
    let Some(address) = state.gateway.address().await else {
        return Ok(());
    };
    let snapshot = super::state::build_local_runtime_state(state)
        .await
        .map_err(|error| LocalPoolError::new(error.code, error.message))?;
    let key = state
        .store()?
        .keys()
        .iter()
        .find(|key| key.system && key.enabled)
        .cloned();
    let key = key.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::Conflict,
            "The local pool request key is unavailable; reconnect Claude Desktop",
        )
    })?;
    let secret = secret_store::load(&key.secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::Conflict,
            "The local pool request key is unavailable; reconnect Claude Desktop",
        )
    })?;
    let models = models::inference_models(&snapshot.gateway.models)?;
    if models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "The pool has no enabled models available to Claude Desktop; the previous Desktop profile was kept",
        ));
    }
    let config = models::gateway_config(&format!("http://{address}"), &secret, models);
    library::apply(&path, &recovery, &config, false)?;
    Ok(())
}

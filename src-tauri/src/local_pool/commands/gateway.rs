use super::runtime_from_store;
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    models::LocalPoolSnapshot,
    state::DesktopState,
};
use tauri::State;

#[tauri::command]
pub async fn start_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let runtime = runtime_from_store(&state)?;
    let port = state.store()?.gateway().port;
    state.gateway.start(runtime, port).await?;
    let enable_result = { state.store()?.set_gateway_enabled(true) };
    if let Err(error) = enable_result {
        state.gateway.stop().await;
        return Err(error.into());
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn stop_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    state.store()?.set_gateway_enabled(false)?;
    state.gateway.stop().await;
    state.snapshot().await.map_err(Into::into)
}

pub async fn start_if_enabled(state: &DesktopState) -> Result<(), LocalPoolError> {
    let (enabled, port) = {
        let store = state.store()?;
        (store.gateway().enabled, store.gateway().port)
    };
    if enabled {
        state
            .gateway
            .start(runtime_from_store(state)?, port)
            .await?;
    }
    Ok(())
}

#[allow(dead_code)]
fn gateway_not_running() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::GatewayUnavailable,
        "local gateway is not running",
    )
}

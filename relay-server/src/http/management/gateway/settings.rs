use super::super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use crate::store::Store;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;
use zenith_relay_core::protocol::RuntimeStateSnapshot;
use zenith_relay_core::GatewayRuntime;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexBackgroundTasksInput {
    enabled: bool,
}

pub async fn set_codex_background_tasks(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CodexBackgroundTasksInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    commit_runtime_flag(
        &state,
        input.enabled,
        Store::codex_background_tasks_enabled,
        Store::set_codex_background_tasks_enabled,
        GatewayRuntime::set_codex_background_tasks_enabled,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatgptRetryUntilAvailableInput {
    enabled: bool,
}

pub async fn set_chatgpt_retry_until_available(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ChatgptRetryUntilAvailableInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    commit_runtime_flag(
        &state,
        input.enabled,
        Store::chatgpt_retry_until_available,
        Store::set_chatgpt_retry_until_available,
        GatewayRuntime::set_route_recovery_enabled,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexWebsocketsInput {
    enabled: bool,
}

pub async fn set_codex_websockets(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CodexWebsocketsInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    commit_runtime_flag(
        &state,
        input.enabled,
        Store::codex_websockets_enabled,
        Store::set_codex_websockets_enabled,
        GatewayRuntime::set_codex_websockets_enabled,
    )
}

fn commit_runtime_flag(
    state: &Arc<AppState>,
    enabled: bool,
    read: fn(&Store) -> Result<bool, String>,
    write: fn(&Store, bool) -> Result<(), String>,
    apply: fn(&GatewayRuntime, bool),
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let previous = read(state.store.as_ref()).map_err(store_error)?;
    write(state.store.as_ref(), enabled).map_err(store_error)?;
    let runtime = match state.runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = write(state.store.as_ref(), previous);
            return Err(runtime_error(error));
        }
    };
    if let Some(runtime) = runtime {
        apply(runtime.as_ref(), enabled);
    }
    if let Err(error) = state.snapshot() {
        let _ = write(state.store.as_ref(), previous);
        if let Some(runtime) = state.runtime().ok().flatten() {
            apply(runtime.as_ref(), previous);
        }
        return Err(runtime_error(error));
    }
    Ok(Json(state.snapshot().map_err(store_error)?))
}

use super::super::{
    fence_runtime_candidates, restart_after_secret_change, runtime_from_store,
    sync_gateway_or_rollback,
};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    models::LocalPoolSnapshot,
    state::DesktopState,
    store::secret_store,
};
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn start_local_gateway(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let result = async {
        let _mutation = state.setup_guard().await;
        let runtime = runtime_from_store(&state).await?;
        let port = state.store()?.gateway().port;
        state.gateway.start(runtime, port).await?;
        let result = super::super::profiles::refresh_active_client_catalogs(&state).await;
        super::super::record_catalog_refresh_result(&state, &result);
        let enable_result = { state.store()?.set_gateway_enabled(true) };
        if let Err(error) = enable_result {
            state.gateway.stop().await;
            return Err(error.into());
        }
        state.snapshot().await.map_err(Into::into)
    }
    .await;
    crate::tray::refresh_tray(&app).await;
    result
}

#[tauri::command]
pub async fn stop_local_gateway(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let result = async {
        let _mutation = state.setup_guard().await;
        state.store()?.set_gateway_enabled(false)?;
        state.gateway.stop().await;
        state.snapshot().await.map_err(Into::into)
    }
    .await;
    crate::tray::refresh_tray(&app).await;
    result
}

#[tauri::command]
pub async fn restart_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    if state.gateway.address().await.is_none() {
        return Err(gateway_not_running().into());
    }
    let gateway = state.store()?.gateway().clone();
    sync_gateway_or_rollback(&state, gateway).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn update_local_gateway_port(
    port: u16,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    if old_gateway.port == port {
        return state.snapshot().await.map_err(Into::into);
    }
    let mut gateway = old_gateway.clone();
    gateway.port = port;
    state.store()?.replace_gateway(gateway)?;
    sync_gateway_or_rollback(&state, old_gateway).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn reveal_local_gateway_api_key(
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    let key = super::super::pool::ensure_system_gateway_key(&state)?;
    Ok(super::super::pool::ensure_local_gateway_key_secret(&key)?)
}

#[tauri::command]
pub async fn rotate_local_gateway_api_key(
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    rotate_system_gateway_api_key(&state)
        .await
        .map_err(Into::into)
}

async fn rotate_system_gateway_api_key(
    state: &DesktopState,
) -> crate::local_pool::error::Result<String> {
    let key = super::super::pool::ensure_system_gateway_key(state)?;
    let old_secret = super::super::pool::ensure_local_gateway_key_secret(&key)?;
    let new_secret = super::super::pool::new_local_gateway_api_key();
    // The principal changes before the replacement listener starts. Do not
    // let requests authenticated with the old secret dispatch in that gap.
    let (account_ids, source_ids) = {
        let store = state.store()?;
        (
            store
                .accounts()
                .iter()
                .map(|account| account.account.id.clone())
                .collect::<Vec<_>>(),
            store
                .sources()
                .iter()
                .map(|source| source.id.clone())
                .collect::<Vec<_>>(),
        )
    };
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &account_ids, &source_ids);
    secret_store::save(&key.secret_ref, &new_secret)?;
    restart_after_secret_change(state, &key.secret_ref, &old_secret).await?;
    Ok(new_secret)
}

pub async fn start_if_enabled(state: &DesktopState) -> Result<(), LocalPoolError> {
    let (enabled, port) = {
        let store = state.store()?;
        (store.gateway().enabled, store.gateway().port)
    };
    if enabled {
        state
            .gateway
            .start(runtime_from_store(state).await?, port)
            .await?;
        // Quota workers can finish between runtime construction and listener
        // creation. Reconcile the persisted account snapshots now that a live
        // scheduler exists so startup cannot strand fresh provider credits.
        super::super::sync_running_account_states(state).await?;
        if state.background_session_active() {
            let result = super::super::profiles::refresh_active_client_catalogs(state).await;
            super::super::record_catalog_refresh_result(state, &result);
        }
    }
    Ok(())
}

pub(super) fn gateway_not_running() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::GatewayUnavailable,
        "local gateway is not running",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rotating_the_local_gateway_key_replaces_the_stored_secret() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-key-rotation-{id}"));
        let state = DesktopState::open(root.clone()).unwrap();
        let key = super::super::super::pool::ensure_system_gateway_key(&state).unwrap();
        let old_secret = super::super::super::pool::ensure_local_gateway_key_secret(&key).unwrap();

        let new_secret = rotate_system_gateway_api_key(&state).await.unwrap();

        assert_ne!(new_secret, old_secret);
        assert_eq!(
            secret_store::load(&key.secret_ref).unwrap().as_deref(),
            Some(new_secret.as_str())
        );

        secret_store::delete(&key.secret_ref).unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

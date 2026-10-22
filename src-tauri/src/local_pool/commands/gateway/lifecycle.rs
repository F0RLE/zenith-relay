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
    let start_result = async {
        let _mutation = state.setup_guard().await;
        let runtime = runtime_from_store(&state).await?;
        let gateway = state.store()?.gateway().clone();
        state
            .gateway
            .start_with_scope(runtime, gateway.port, gateway.bind_scope)
            .await?;
        let enable_result = { state.store()?.set_gateway_enabled(true) };
        if let Err(error) = enable_result {
            state.gateway.stop().await;
            return Err(error.into());
        }
        super::super::queue_active_client_catalog_refresh(&state);
        state.snapshot().await.map_err(Into::into)
    }
    .await;
    crate::tray::refresh_tray(&app).await;
    start_result
}

#[tauri::command]
pub async fn stop_local_gateway(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let stop_result = async {
        let _mutation = state.setup_guard().await;
        state.store()?.set_gateway_enabled(false)?;
        state.gateway.stop().await;
        state.snapshot().await.map_err(Into::into)
    }
    .await;
    crate::tray::refresh_tray(&app).await;
    stop_result
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
    network: Option<bool>,
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    let key = if network.unwrap_or(false) {
        super::super::pool::ensure_lan_gateway_key(&state)?
    } else {
        super::super::pool::ensure_system_gateway_key(&state)?
    };
    Ok(super::super::pool::ensure_local_gateway_key_secret(&key)?)
}

#[tauri::command]
pub async fn rotate_local_gateway_api_key(
    network: Option<bool>,
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    rotate_gateway_api_key(&state, network.unwrap_or(false))
        .await
        .map_err(Into::into)
}

#[cfg(test)]
async fn rotate_system_gateway_api_key(
    state: &DesktopState,
) -> crate::local_pool::error::Result<String> {
    rotate_gateway_api_key(state, false).await
}

async fn rotate_gateway_api_key(
    state: &DesktopState,
    network: bool,
) -> crate::local_pool::error::Result<String> {
    let key = if network {
        super::super::pool::ensure_lan_gateway_key(state)?
    } else {
        super::super::pool::ensure_system_gateway_key(state)?
    };
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
    let _mutation = state.setup_guard().await;
    let (enabled, port, scope) = {
        let store = state.store()?;
        (
            store.gateway().enabled,
            store.gateway().port,
            store.gateway().bind_scope,
        )
    };
    if enabled {
        state
            .gateway
            .start_with_scope(runtime_from_store(state).await?, port, scope)
            .await?;
        // Quota workers can finish between runtime construction and listener
        // creation. Reconcile the persisted account snapshots now that a live
        // scheduler exists so startup cannot strand fresh provider credits.
        super::super::sync_running_account_states(state).await?;
        if state.background_session_active() {
            super::super::queue_active_client_catalog_refresh(state);
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
    async fn startup_waits_for_settings_transaction_before_reading_enabled_state() {
        let root =
            std::env::temp_dir().join(format!("relay-startup-lock-{}", uuid::Uuid::new_v4()));
        let state = DesktopState::open(root.clone()).unwrap();
        state.store().unwrap().set_gateway_enabled(false).unwrap();
        let mutation = state.setup_guard().await;
        let mut startup = Box::pin(start_if_enabled(&state));
        let completed_while_locked =
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut startup)
                .await
                .is_ok();
        drop(mutation);
        if !completed_while_locked {
            startup.as_mut().await.unwrap();
        }
        drop(startup);
        assert!(state.gateway.address().await.is_none());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            !completed_while_locked,
            "startup bypassed the settings transaction"
        );
    }

    #[tokio::test]
    async fn rotating_the_local_gateway_key_replaces_the_stored_secret() {
        let temp_id = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("zenith-relay-key-rotation-{temp_id}"));
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

    #[tokio::test]
    async fn rotating_network_key_preserves_local_application_key() {
        let root = std::env::temp_dir().join(format!("relay-network-key-{}", uuid::Uuid::new_v4()));
        let state = DesktopState::open(root.clone()).unwrap();
        let pool = super::super::super::pool::ensure_system_gateway_key(&state).unwrap();
        let local_secret =
            super::super::super::pool::ensure_local_gateway_key_secret(&pool).unwrap();
        let network = super::super::super::pool::ensure_lan_gateway_key(&state).unwrap();
        let old_secret =
            super::super::super::pool::ensure_local_gateway_key_secret(&network).unwrap();
        let new_secret = rotate_gateway_api_key(&state, true).await.unwrap();
        assert_ne!(old_secret, new_secret);
        assert_eq!(
            secret_store::load(&network.secret_ref).unwrap(),
            Some(new_secret)
        );
        assert_eq!(
            secret_store::load(&pool.secret_ref).unwrap(),
            Some(local_secret)
        );
        secret_store::delete(&pool.secret_ref).unwrap();
        secret_store::delete(&network.secret_ref).unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

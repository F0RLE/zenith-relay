use super::super::super::{
    error::{ErrorCode, ErrorDiagnostics, LocalPoolError, Result},
    models::{GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use super::super::profiles;
use super::assemble::runtime_from_store;
use super::{current_time_ms, record_catalog_refresh_result, runtime_account_operational_state};
use tauri::{AppHandle, Emitter, Manager};
use zenith_relay_core::{protocol::account_candidate_enabled, GatewayRuntime};

pub(in crate::local_pool) fn refresh_active_codex_catalog_in_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        let catalog_refresh_result = profiles::refresh_active_client_catalogs(&state).await;
        record_catalog_refresh_result(&state, &catalog_refresh_result);
        let _ = app.emit("zenith-state-changed", ());
    });
}

pub(in crate::local_pool) async fn sync_records_or_rollback(
    state: &DesktopState,
    old_sources: Vec<ProviderSourceRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> Result<()> {
    restart_or_rollback(state, || {
        state.store()?.replace_records(old_sources, old_keys)
    })
    .await
}

pub(in crate::local_pool) async fn sync_account_or_rollback(
    state: &DesktopState,
    previous_account: LocalAccountRecord,
    attempted_account: LocalAccountRecord,
) -> Result<()> {
    restart_or_rollback(state, move || {
        state
            .store()?
            .restore_account_if_current(&previous_account, &attempted_account)
            .map(|_| ())
    })
    .await
}

/// Reconciles account policy and quota snapshots that may have changed while
/// the startup runtime was being constructed. Automatic quota refreshes run
/// independently of Gateway startup, so a refresh can finish before the
/// listener exists and otherwise have no live scheduler to update.
pub(in crate::local_pool) async fn sync_running_account_states(state: &DesktopState) -> Result<()> {
    let Some(runtime) = state.gateway.runtime().await else {
        return Ok(());
    };
    let accounts = state.store()?.accounts().to_vec();
    let observed_at_ms = current_time_ms();
    for account in accounts {
        sync_runtime_account_state(&runtime, &account, observed_at_ms);
    }
    Ok(())
}

pub(in crate::local_pool) async fn sync_refreshed_account_or_rollback(
    state: &DesktopState,
    previous_account: LocalAccountRecord,
    attempted_account: LocalAccountRecord,
    models_changed: bool,
) -> Result<()> {
    let account_id = attempted_account.account.id.clone();
    let Some(runtime) = state.gateway.runtime().await else {
        return Ok(());
    };
    let account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let observed_at_ms = current_time_ms();
    let health_changed =
        runtime_account_operational_state(&previous_account.account, observed_at_ms).health
            != runtime_account_operational_state(&account.account, observed_at_ms).health;
    if (!models_changed || runtime.update_account_models(&account_id, account.effective_models()))
        && sync_runtime_account_state_with_live_block(
            &runtime,
            &account,
            observed_at_ms,
            !health_changed,
        )
    {
        return Ok(());
    }
    sync_account_or_rollback(state, previous_account, attempted_account).await
}

pub(in crate::local_pool) fn sync_runtime_account_state(
    runtime: &GatewayRuntime,
    account: &LocalAccountRecord,
    observed_at_ms: u64,
) -> bool {
    sync_runtime_account_state_with_live_block(runtime, account, observed_at_ms, false)
}

fn sync_runtime_account_state_with_live_block(
    runtime: &GatewayRuntime,
    account: &LocalAccountRecord,
    observed_at_ms: u64,
    preserve_live_block: bool,
) -> bool {
    let operational = runtime_account_operational_state(&account.account, observed_at_ms);
    let enabled =
        account_candidate_enabled(account.account.enabled, operational.routing_block_reason);
    if preserve_live_block {
        runtime.sync_account_refresh_availability_with_quota(
            &account.account.id,
            enabled,
            operational.health,
            &account.account.quota,
            observed_at_ms,
        )
    } else {
        runtime.sync_account_availability_with_quota(
            &account.account.id,
            enabled,
            operational.health,
            &account.account.quota,
            observed_at_ms,
        )
    }
}

pub(in crate::local_pool) async fn sync_gateway_or_rollback(
    state: &DesktopState,
    old_gateway: GatewaySettings,
) -> Result<()> {
    restart_or_rollback(state, || state.store()?.replace_gateway(old_gateway)).await
}

pub(in crate::local_pool) async fn restart_after_secret_change(
    state: &DesktopState,
    secret_ref: &str,
    old_secret: &str,
) -> Result<()> {
    restart_or_rollback(state, || secret_store::save(secret_ref, old_secret)).await
}

pub(in crate::local_pool) async fn restart_or_rollback(
    state: &DesktopState,
    rollback: impl FnOnce() -> Result<()> + Send,
) -> Result<()> {
    crate::diagnostics::breadcrumb("gateway-runtime", "restart_started", &[]);
    let Some(address) = state.gateway.address().await else {
        crate::diagnostics::breadcrumb("gateway-runtime", "restart_skipped", &[]);
        return Ok(());
    };
    let next_port = state.store()?.gateway().port;
    let mut rollback = Some(rollback);
    let runtime = match runtime_from_store(state).await {
        Ok(runtime) => runtime,
        Err(error) => {
            crate::diagnostics::record_error(
                "gateway-runtime",
                Some("runtime_rebuild_failed"),
                &error.message,
                &[],
            );
            let Some(rollback) = rollback.take() else {
                return Err(fail_closed(
                    state,
                    format!("{error}; gateway rollback callback was consumed unexpectedly"),
                )
                .await);
            };
            apply_rollback(state, rollback).await?;
            // A caller may already have hot-applied one member before it fell
            // back to a full rebuild. Restoring only the durable records
            // would leave that old runtime with newer permissions once its
            // dispatch fences are released.
            let restored = match runtime_from_store(state).await {
                Ok(runtime) => runtime,
                Err(restore) => {
                    return Err(fail_closed(
                        state,
                        format!("{error}; failed to rebuild restored gateway: {restore}"),
                    )
                    .await)
                }
            };
            if let Some(previous_runtime) = state.gateway.runtime().await {
                previous_runtime.retire_for_replacement();
            }
            state.gateway.stop().await;
            if let Err(restart) = state.gateway.start(restored, address.port()).await {
                return Err(fail_closed(
                    state,
                    format!("{error}; failed to restart restored gateway: {restart}"),
                )
                .await);
            }
            return Err(error);
        }
    };

    crate::diagnostics::breadcrumb(
        "gateway-runtime",
        "gateway_stop_started",
        &[("port", address.port().to_string())],
    );
    // A delayed request may still hold the old Arc after the listener stops.
    // Failed activation rebuilds a fresh runtime from restored storage.
    if let Some(previous_runtime) = state.gateway.runtime().await {
        previous_runtime.retire_for_replacement();
    }
    state.gateway.stop().await;
    crate::diagnostics::breadcrumb(
        "gateway-runtime",
        "gateway_start_started",
        &[("port", next_port.to_string())],
    );
    let restart_error = state.gateway.start(runtime, next_port).await.err();
    if let Some(error) = restart_error {
        crate::diagnostics::record_error(
            "gateway-runtime",
            Some("gateway_restart_failed"),
            &error.to_string(),
            &[],
        );
        state.gateway.stop().await;
        let Some(rollback) = rollback.take() else {
            return Err(fail_closed(
                state,
                format!("{error}; gateway rollback callback was consumed unexpectedly"),
            )
            .await);
        };
        apply_rollback(state, rollback).await?;
        let old_runtime = match runtime_from_store(state).await {
            Ok(runtime) => runtime,
            Err(restore) => {
                return Err(fail_closed(
                    state,
                    format!("{error}; failed to rebuild previous gateway: {restore}"),
                )
                .await)
            }
        };
        if let Err(restart) = state.gateway.start(old_runtime, address.port()).await {
            return Err(fail_closed(
                state,
                format!("{error}; failed to restart previous gateway: {restart}"),
            )
            .await);
        }
        return Err(error);
    }
    crate::diagnostics::breadcrumb(
        "gateway-runtime",
        "gateway_started",
        &[("port", next_port.to_string())],
    );
    crate::diagnostics::breadcrumb("gateway-runtime", "catalog_refresh_started", &[]);
    let catalog_refresh_result = profiles::refresh_active_client_catalogs(state).await;
    record_catalog_refresh_result(state, &catalog_refresh_result);
    crate::diagnostics::record_operation("gateway-runtime", "restart_completed", &[]);
    Ok(())
}

async fn apply_rollback(state: &DesktopState, rollback: impl FnOnce() -> Result<()>) -> Result<()> {
    if let Err(error) = rollback() {
        return Err(fail_closed(
            state,
            format!("failed to restore previous gateway state: {error}"),
        )
        .await);
    }
    Ok(())
}

fn disable_gateway(state: &DesktopState) -> Result<()> {
    state.store()?.set_gateway_enabled(false)
}

pub(in crate::local_pool) async fn fail_closed(
    state: &DesktopState,
    message: String,
) -> LocalPoolError {
    crate::diagnostics::record_error("gateway-runtime", Some("fail_closed"), &message, &[]);
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.retire_for_replacement();
    }
    state.gateway.stop().await;
    match disable_gateway(state) {
        Ok(()) => LocalPoolError::new(ErrorCode::RecoveryRequired, message),
        Err(error) => LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{message}; failed to disable gateway state: {error}"),
        ),
    }
}

pub(in crate::local_pool) fn core_error(error: zenith_relay_core::Error) -> LocalPoolError {
    let message = error.to_string();
    let code = match &error {
        zenith_relay_core::Error::ManagementHttpUnavailable
        | zenith_relay_core::Error::Upstream(_)
        | zenith_relay_core::Error::UpstreamBodyTooLarge
        | zenith_relay_core::Error::UpstreamStatus(_)
        | zenith_relay_core::Error::InvalidUpstreamResponse(_) => ErrorCode::SourceTestFailed,
        zenith_relay_core::Error::Validation(_) | zenith_relay_core::Error::UnsupportedWireApi => {
            ErrorCode::InvalidState
        }
    };
    let mut local_error = LocalPoolError::new(code, message);
    if let zenith_relay_core::Error::UpstreamStatus(status) = error {
        local_error = local_error.with_diagnostic(ErrorDiagnostics {
            status: Some(status),
            retryable: Some(status == 408 || status == 429 || status >= 500),
            ..Default::default()
        });
    }
    local_error
}

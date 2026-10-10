use super::{now_ms, ConnectRemoteServerInput, RemoteConnectionState};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    remote::{self, client::RemoteClient, RemoteTargetRecord},
    state::DesktopState,
};
use sha2::{Digest, Sha256};
use tauri::State;
use zenith_relay_core::protocol::{GatewayDiagnostic, RuntimeStateSnapshot, UsagePage, UsageQuery};
use zenith_relay_core::{CandidateRuntimeSnapshot, SourceProviderStats};

#[tauri::command]
pub async fn get_remote_source_stats(
    source_id: String,
    force: Option<bool>,
    state: State<'_, DesktopState>,
) -> Result<SourceProviderStats, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    client
        .source_stats(&source_id, force.unwrap_or(false))
        .await
        .map_err(remote_error)
}

#[tauri::command]
pub async fn connect_remote_server(
    input: ConnectRemoteServerInput,
    state: State<'_, DesktopState>,
) -> Result<RemoteConnectionState, CommandError> {
    let _mutation = state.setup_guard().await;
    let client = RemoteClient::new(
        &input.base_url,
        &input.management_token,
        input.allow_insecure_http,
    )
    .map_err(remote_error)?;
    let (health, capabilities, negotiated) = client.negotiate().await.map_err(remote_error)?;
    if health.server_id != negotiated.server_id {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "remote health and capabilities identities do not match",
        )
        .into());
    }
    let pending_operation = state.store()?.ownership_operation().cloned();
    if pending_operation
        .as_ref()
        .is_some_and(|operation| operation.server_id != negotiated.server_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "the pending account ownership operation belongs to another server",
        )
        .into());
    }
    let needs_reconciliation = pending_operation.is_none()
        && state.store()?.accounts().iter().any(|account| {
            account
                .remote_location
                .as_ref()
                .is_some_and(|location| location.server_id == negotiated.server_id)
        });
    let remote_snapshot = if needs_reconciliation {
        Some(client.state().await.map_err(remote_error)?)
    } else {
        None
    };
    let previous_target = state.store()?.remote_target().cloned();
    if previous_target.as_ref().is_some_and(|stored_target| {
        same_origin_identity_changed(
            stored_target,
            client.origin(),
            &negotiated.server_id,
            &negotiated.identity_fingerprint,
        )
    }) && !input.confirm_identity_change
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "remote server identity changed; explicit confirmation is required",
        )
        .into());
    }
    let secret_ref = remote_secret_ref(client.origin());
    let target = RemoteTargetRecord {
        origin: client.origin().to_string(),
        server_id: negotiated.server_id,
        identity_fingerprint: negotiated.identity_fingerprint,
        server_version: health.version.clone(),
        protocol_version: negotiated.version,
        allow_insecure_http: input.allow_insecure_http,
        secret_ref,
        connected_at_ms: now_ms(),
    };
    let previous_same_secret = previous_target
        .as_ref()
        .filter(|stored_target| stored_target.secret_ref == target.secret_ref)
        .and_then(|stored_target| remote::load_token(stored_target).ok().flatten());
    remote::save_token(&target, &input.management_token)?;
    if let Err(error) = state.store()?.replace_remote_target(Some(target.clone())) {
        match previous_same_secret {
            Some(token) => {
                let _ = remote::save_token(&target, &token);
            }
            None => {
                let _ = remote::delete_token(&target);
            }
        }
        return Err(error.into());
    }
    if let Some(previous_target) = previous_target {
        if previous_target.secret_ref != target.secret_ref {
            let _ = remote::delete_token(&previous_target);
        }
    }
    if let Some(snapshot) = &remote_snapshot {
        super::ownership::reconcile_remote_account_locations(&state, &target, snapshot).await?;
    }
    Ok(RemoteConnectionState {
        target,
        health,
        capabilities,
    })
}

#[tauri::command]
pub async fn get_remote_server_state(
    state: State<'_, DesktopState>,
) -> Result<Option<RuntimeStateSnapshot>, CommandError> {
    super::recover_pending_remote_ownership(&state).await?;
    let _mutation = state.setup_guard().await;
    let Some((target, client)) = active_client(&state)? else {
        return Ok(None);
    };
    let snapshot = client.state().await.map_err(remote_error)?;
    super::ownership::reconcile_remote_account_locations(&state, &target, &snapshot).await?;
    Ok(Some(snapshot))
}

#[tauri::command]
pub async fn get_remote_runtime_order(
    state: State<'_, DesktopState>,
) -> Result<Option<Vec<CandidateRuntimeSnapshot>>, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Ok(None);
    };
    client.runtime_order().await.map(Some).map_err(remote_error)
}

#[tauri::command]
pub async fn get_remote_server_usage(
    input: Option<UsageQuery>,
    state: State<'_, DesktopState>,
) -> Result<Option<UsagePage>, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Ok(None);
    };
    client
        .usage(&input.unwrap_or_default())
        .await
        .map(Some)
        .map_err(remote_error)
}

#[tauri::command]
pub async fn diagnose_remote_gateway(
    stream: bool,
    state: State<'_, DesktopState>,
) -> Result<GatewayDiagnostic, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    client.diagnose(stream).await.map_err(remote_error)
}

#[tauri::command]
pub async fn refresh_remote_server_capabilities(
    state: State<'_, DesktopState>,
) -> Result<Option<RemoteConnectionState>, CommandError> {
    let Some((mut target, client)) = active_client(&state)? else {
        return Ok(None);
    };
    let (health, capabilities, negotiated) = client.negotiate().await.map_err(remote_error)?;
    if negotiated.identity_fingerprint != target.identity_fingerprint
        || negotiated.server_id != target.server_id
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "remote server identity changed; reconnect with explicit confirmation",
        )
        .into());
    }
    target.server_version = health.version.clone();
    target.protocol_version = negotiated.version;
    state.store()?.replace_remote_target(Some(target.clone()))?;
    Ok(Some(RemoteConnectionState {
        target,
        health,
        capabilities,
    }))
}

#[tauri::command]
pub async fn disconnect_remote_server(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    super::ownership::ensure_no_pending_ownership_operation(&state)?;
    let Some(target) = state.store()?.remote_target().cloned() else {
        return Ok(());
    };
    let token = remote::load_token(&target)?;
    remote::delete_token(&target)?;
    if let Err(error) = state.store()?.replace_remote_target(None) {
        if let Some(token) = token {
            let _ = remote::save_token(&target, &token);
        }
        return Err(error.into());
    }
    Ok(())
}

pub(in crate::local_pool::commands) fn active_client(
    state: &DesktopState,
) -> Result<Option<(RemoteTargetRecord, RemoteClient)>, CommandError> {
    let Some(target) = state.store()?.remote_target().cloned() else {
        return Ok(None);
    };
    let token = remote::load_token(&target)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "remote management token is unavailable",
        )
    })?;
    let client = RemoteClient::new(&target.origin, &token, target.allow_insecure_http)
        .map_err(remote_error)?;
    Ok(Some((target, client)))
}

fn remote_secret_ref(origin: &str) -> String {
    format!("remote:{}", hex::encode(Sha256::digest(origin.as_bytes())))
}

fn same_origin_identity_changed(
    previous_target: &RemoteTargetRecord,
    origin: &str,
    server_id: &str,
    identity_fingerprint: &str,
) -> bool {
    previous_target.origin == origin
        && (previous_target.server_id != server_id
            || previous_target.identity_fingerprint != identity_fingerprint)
}

pub(in crate::local_pool::commands) fn remote_error(error: impl std::fmt::Display) -> CommandError {
    LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_server_id_or_fingerprint_change_requires_confirmation() {
        let target = RemoteTargetRecord {
            origin: "https://relay.example.test".into(),
            server_id: "server-one".into(),
            identity_fingerprint: "fingerprint-one".into(),
            server_version: "1.1.0".into(),
            protocol_version: 2,
            allow_insecure_http: false,
            secret_ref: "remote:test".into(),
            connected_at_ms: 1,
        };

        assert!(same_origin_identity_changed(
            &target,
            &target.origin,
            "server-two",
            &target.identity_fingerprint,
        ));
        assert!(same_origin_identity_changed(
            &target,
            &target.origin,
            &target.server_id,
            "fingerprint-two",
        ));
        assert!(!same_origin_identity_changed(
            &target,
            "https://other.example.test",
            "server-two",
            "fingerprint-two",
        ));
    }
}

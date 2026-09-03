use crate::local_pool::{
    accounts::{
        exports::{
            finish_account_export, normalize_account_ids, AccountExportInput, AccountExportResult,
        },
        import_orchestrator::{pick_account_import_documents, read_import_documents},
    },
    error::{CommandError, ErrorCode, LocalPoolError},
    remote::{
        self,
        client::RemoteClient,
        deployment::{self, DeploymentPlan},
        RemoteTargetRecord,
    },
    state::DesktopState,
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use zenith_relay_core::accounts::AccountExportRequest;
use zenith_relay_core::protocol::{
    Capabilities, ConfigurationPreset, ConfigurationPresetApplyInput,
    ConfigurationPresetApplyResult, ConfigurationPresetPreview, ConfigurationPresetPreviewInput,
    GatewayDiagnostic, HealthResponse, RevealedAccountIdentity, RuntimeStateSnapshot, UsagePage,
    UsageQuery,
};
use zenith_relay_core::{CandidateRuntimeSnapshot, SourceProviderStats};

pub(super) use zenith_relay_core::unix_time_ms as now_ms;

use super::pool::write_configuration_preset;

mod ownership;

pub(crate) use ownership::{reconcile_saved_remote_ownership, recover_pending_remote_ownership};
pub use ownership::{
    ForceActivateRemoteAccountLocallyInput, ForceActivateRemoteAccountLocallyResult,
    MoveLocalAccountsToRemoteInput, MoveLocalAccountsToRemoteResult,
    ReturnRemoteAccountToLocalInput, ReturnRemoteAccountToLocalResult,
};

const MAX_CONFIGURATION_PRESET_BYTES: usize = 1024 * 1024;
#[tauri::command]
pub async fn get_remote_source_stats(
    source_id: String,
    state: State<'_, DesktopState>,
) -> Result<SourceProviderStats, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    client.source_stats(&source_id).await.map_err(remote_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRemoteServerInput {
    pub base_url: String,
    pub management_token: String,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub confirm_identity_change: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnectionState {
    pub target: RemoteTargetRecord,
    pub health: HealthResponse,
    pub capabilities: Capabilities,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareRemoteDeploymentInput {
    pub public_base_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RemoteServerAction {
    CreateSource,
    UpdateSource { id: String },
    DeleteSource { id: String },
    TestSource { id: String },
    PreviewAccountImport,
    ConfirmAccountImport,
    PreviewAccountBatchImport,
    ConfirmAccountBatchImport,
    UpdateAccount { id: String },
    RefreshAccount { id: String },
    DeleteAccount { id: String },
    SetCommonProxy,
    SetAccountProxyRequired,
    SetAccountProxy { id: String },
    AssignAccountProxies,
    SetPoolMembership,
    SetQuotaPolicy,
    SetRoutingPolicy,
    RefreshAllQuotas,
    RefreshPricingCatalog,
    SetModelEnabled,
    SetModelPrice,
    SetModelReasoning,
    SetModelServiceTier,
    SetModelOrder,
    StartGateway,
    StopGateway,
    SetCodexBackgroundTasks,
    SetCodexWebsockets,
    CreateWakeTask,
    UpdateWakeTask { id: String },
    DeleteWakeTask { id: String },
    TestWakeTask { id: String },
    ClearUsage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteRemoteServerActionInput {
    pub action: RemoteServerAction,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
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
    let previous = state.store()?.remote_target().cloned();
    if previous.as_ref().is_some_and(|record| {
        same_origin_identity_changed(
            record,
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
    let previous_same_secret = previous
        .as_ref()
        .filter(|record| record.secret_ref == target.secret_ref)
        .and_then(|record| remote::load_token(record).ok().flatten());
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
    if let Some(previous) = previous {
        if previous.secret_ref != target.secret_ref {
            let _ = remote::delete_token(&previous);
        }
    }
    if let Some(snapshot) = &remote_snapshot {
        ownership::reconcile_remote_account_locations(&state, &target, snapshot)?;
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
    recover_pending_remote_ownership(&state).await?;
    let _mutation = state.setup_guard().await;
    let Some((target, client)) = active_client(&state)? else {
        return Ok(None);
    };
    let snapshot = client.state().await.map_err(remote_error)?;
    ownership::reconcile_remote_account_locations(&state, &target, &snapshot)?;
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
pub async fn reveal_remote_gateway_api_key(
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    Ok(client
        .profile_credential()
        .await
        .map_err(remote_error)?
        .secret)
}

#[tauri::command]
pub async fn rotate_remote_gateway_api_key(
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let rotation = client
        .prepare_profile_key_rotation()
        .await
        .map_err(remote_error)?;
    if let Err(error) = client
        .commit_profile_key_rotation(&rotation.rotation_id)
        .await
    {
        let committed = client.profile_credential().await.is_ok_and(|credential| {
            credential.key_id == rotation.key_id
                && credential.base_url == rotation.base_url
                && credential.secret == rotation.secret
        });
        if !committed {
            let _ = client
                .abort_profile_key_rotation(&rotation.rotation_id)
                .await;
            return Err(remote_error(error));
        }
    }
    Ok(rotation.secret)
}

#[tauri::command]
pub async fn export_remote_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<String>, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let document = client.configuration_preset().await.map_err(remote_error)?;
    write_configuration_preset(&document.preset, &app)
}

#[tauri::command]
pub async fn preview_remote_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<ConfigurationPresetPreview>, CommandError> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Zenith Relay configuration", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "selected preset path is invalid")
    })?;
    let content = fs::read(&path).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset could not be read",
        )
    })?;
    if content.len() > MAX_CONFIGURATION_PRESET_BYTES {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset exceeds 1 MiB",
        )
        .into());
    }
    let preset: ConfigurationPreset = serde_json::from_slice(&content).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset is invalid or contains unsupported fields",
        )
    })?;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    client
        .preview_configuration_preset(&ConfigurationPresetPreviewInput { preset })
        .await
        .map(Some)
        .map_err(remote_error)
}

#[tauri::command]
pub async fn apply_remote_configuration_preset(
    input: ConfigurationPresetApplyInput,
    state: State<'_, DesktopState>,
) -> Result<ConfigurationPresetApplyResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    client
        .apply_configuration_preset(&input)
        .await
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
pub async fn export_remote_accounts(
    input: AccountExportInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<AccountExportResult, CommandError> {
    let account_ids = normalize_account_ids(input.account_ids)?;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let document = client
        .export_accounts(&AccountExportRequest {
            account_ids,
            format: input.format,
            description: input.description,
        })
        .await
        .map_err(remote_error)?;
    finish_account_export(document, input.destination, &app)
}

#[tauri::command]
pub async fn reveal_remote_account_identity(
    account_id: String,
    state: State<'_, DesktopState>,
) -> Result<RevealedAccountIdentity, CommandError> {
    let account_id = normalize_account_ids(vec![account_id])?
        .pop()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "account id is required"))?;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    client
        .reveal_account_identity(&account_id)
        .await
        .map_err(remote_error)
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
    ownership::ensure_no_pending_ownership_operation(&state)?;
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

#[tauri::command]
pub fn get_remote_linked_account_count(
    state: State<'_, DesktopState>,
) -> Result<usize, CommandError> {
    let store = state.store()?;
    let Some(target) = store.remote_target() else {
        return Ok(0);
    };
    Ok(store
        .accounts()
        .iter()
        .filter(|account| {
            account
                .remote_location
                .as_ref()
                .is_some_and(|location| location.server_id == target.server_id)
        })
        .count())
}

#[tauri::command]
pub fn prepare_remote_server_deployment(
    input: PrepareRemoteDeploymentInput,
    state: State<'_, DesktopState>,
) -> Result<DeploymentPlan, CommandError> {
    deployment::prepare(&state.output_root(), &input.public_base_url).map_err(Into::into)
}

#[tauri::command]
pub async fn preview_remote_account_import_files(
    paths: Option<Vec<std::path::PathBuf>>,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<serde_json::Value>, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let documents = match paths {
        Some(paths) => Some(read_import_documents(paths)?),
        None => pick_account_import_documents(&app)?,
    };
    let Some(documents) = documents else {
        return Ok(None);
    };
    let payload = serde_json::json!({ "documents": documents });
    let preview = client
        .mutate(
            Method::POST,
            "/accounts/import/batch/preview",
            Some(&payload),
        )
        .await
        .map_err(remote_error)?;
    Ok(Some(preview))
}

// Keep Tauri commands at the stable remote-server boundary. The ownership
// state machine itself lives in a focused submodule.
#[tauri::command]
pub async fn move_local_accounts_to_remote(
    input: MoveLocalAccountsToRemoteInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<MoveLocalAccountsToRemoteResult, CommandError> {
    ownership::move_local_accounts_to_remote(input, app, state).await
}

#[tauri::command]
pub async fn return_remote_account_to_local(
    input: ReturnRemoteAccountToLocalInput,
    state: State<'_, DesktopState>,
) -> Result<ReturnRemoteAccountToLocalResult, CommandError> {
    ownership::return_remote_account_to_local(input, state).await
}

#[tauri::command]
pub async fn force_activate_remote_account_locally(
    input: ForceActivateRemoteAccountLocallyInput,
    state: State<'_, DesktopState>,
) -> Result<ForceActivateRemoteAccountLocallyResult, CommandError> {
    ownership::force_activate_remote_account_locally(input, state).await
}

#[tauri::command]
pub async fn execute_remote_server_action(
    input: ExecuteRemoteServerActionInput,
    state: State<'_, DesktopState>,
) -> Result<serde_json::Value, CommandError> {
    let _mutation = state.setup_guard().await;
    if let RemoteServerAction::DeleteAccount { id } = &input.action {
        if state
            .store()?
            .ownership_operation()
            .is_some_and(|operation| {
                operation
                    .remote_account_ids
                    .iter()
                    .any(|account_id| account_id == id)
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "account ownership recovery must finish before deleting this server record",
            )
            .into());
        }
    }
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let (method, path, requires_payload) = action_request(&input.action)?;
    if requires_payload && input.payload.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote action payload is required",
        )
        .into());
    }
    client
        .mutate(method, &path, input.payload.as_ref())
        .await
        .map_err(remote_error)
}

pub(super) fn active_client(
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
    previous: &RemoteTargetRecord,
    origin: &str,
    server_id: &str,
    identity_fingerprint: &str,
) -> bool {
    previous.origin == origin
        && (previous.server_id != server_id
            || previous.identity_fingerprint != identity_fingerprint)
}

fn action_request(action: &RemoteServerAction) -> Result<(Method, String, bool), CommandError> {
    let request = match action {
        RemoteServerAction::CreateSource => (Method::POST, "/sources".to_string(), true),
        RemoteServerAction::UpdateSource { id } => {
            (Method::PATCH, object_path("sources", id)?, true)
        }
        RemoteServerAction::DeleteSource { id } => {
            (Method::DELETE, object_path("sources", id)?, false)
        }
        RemoteServerAction::TestSource { id } => (
            Method::POST,
            format!("{}/test", object_path("sources", id)?),
            false,
        ),
        RemoteServerAction::PreviewAccountImport => {
            (Method::POST, "/accounts/import/preview".to_string(), true)
        }
        RemoteServerAction::ConfirmAccountImport => {
            (Method::POST, "/accounts/import/confirm".to_string(), true)
        }
        RemoteServerAction::PreviewAccountBatchImport => (
            Method::POST,
            "/accounts/import/batch/preview".to_string(),
            true,
        ),
        RemoteServerAction::ConfirmAccountBatchImport => (
            Method::POST,
            "/accounts/import/batch/confirm".to_string(),
            true,
        ),
        RemoteServerAction::UpdateAccount { id } => {
            (Method::PATCH, object_path("accounts", id)?, true)
        }
        RemoteServerAction::RefreshAccount { id } => (
            Method::POST,
            format!("{}/refresh", object_path("accounts", id)?),
            false,
        ),
        RemoteServerAction::DeleteAccount { id } => {
            (Method::DELETE, object_path("accounts", id)?, false)
        }
        RemoteServerAction::SetCommonProxy => (Method::POST, "/proxies/common".to_string(), true),
        RemoteServerAction::SetAccountProxyRequired => {
            (Method::POST, "/proxies/policy".to_string(), true)
        }
        RemoteServerAction::SetAccountProxy { id } => (
            Method::POST,
            format!("{}/proxy", object_path("accounts", id)?),
            true,
        ),
        RemoteServerAction::AssignAccountProxies => {
            (Method::POST, "/accounts/proxies/assign".to_string(), true)
        }
        RemoteServerAction::SetPoolMembership => (Method::POST, "/pool/members".to_string(), true),
        RemoteServerAction::SetQuotaPolicy => (Method::POST, "/quota/settings".to_string(), true),
        RemoteServerAction::SetRoutingPolicy => {
            (Method::POST, "/routing/settings".to_string(), true)
        }
        RemoteServerAction::RefreshAllQuotas => {
            (Method::POST, "/pool/quota/refresh".to_string(), false)
        }
        RemoteServerAction::RefreshPricingCatalog => {
            (Method::POST, "/pricing/refresh".to_string(), false)
        }
        RemoteServerAction::SetModelEnabled => (Method::POST, "/models/rules".to_string(), true),
        RemoteServerAction::SetModelPrice => (Method::POST, "/models/prices".to_string(), true),
        RemoteServerAction::SetModelReasoning => {
            (Method::POST, "/models/reasoning".to_string(), true)
        }
        RemoteServerAction::SetModelServiceTier => {
            (Method::POST, "/models/service-tier".to_string(), true)
        }
        RemoteServerAction::SetModelOrder => (Method::POST, "/models/order".to_string(), true),
        RemoteServerAction::StartGateway => (Method::POST, "/gateway/start".to_string(), false),
        RemoteServerAction::StopGateway => (Method::POST, "/gateway/stop".to_string(), false),
        RemoteServerAction::SetCodexBackgroundTasks => (
            Method::POST,
            "/gateway/codex-background-tasks".to_string(),
            true,
        ),
        RemoteServerAction::SetCodexWebsockets => {
            (Method::POST, "/gateway/codex-websockets".to_string(), true)
        }
        RemoteServerAction::CreateWakeTask => (Method::POST, "/wake-tasks".to_string(), true),
        RemoteServerAction::UpdateWakeTask { id } => {
            (Method::PATCH, object_path("wake-tasks", id)?, true)
        }
        RemoteServerAction::DeleteWakeTask { id } => {
            (Method::DELETE, object_path("wake-tasks", id)?, false)
        }
        RemoteServerAction::TestWakeTask { id } => (
            Method::POST,
            format!("{}/test", object_path("wake-tasks", id)?),
            false,
        ),
        RemoteServerAction::ClearUsage => (Method::DELETE, "/usage".to_string(), false),
    };
    Ok(request)
}

fn object_path(collection: &str, id: &str) -> Result<String, CommandError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            LocalPoolError::new(ErrorCode::InvalidState, "remote object id is invalid").into(),
        );
    }
    Ok(format!("/{collection}/{id}"))
}

pub(super) fn remote_error(error: impl std::fmt::Display) -> CommandError {
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

    #[test]
    fn pricing_catalog_refresh_uses_its_dedicated_empty_post_request() {
        let (method, path, requires_payload) =
            action_request(&RemoteServerAction::RefreshPricingCatalog)
                .expect("pricing catalog refresh request should be supported");

        assert_eq!(method, Method::POST);
        assert_eq!(path, "/pricing/refresh");
        assert!(!requires_payload);
    }
}

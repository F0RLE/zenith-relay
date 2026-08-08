use crate::local_pool::{
    accounts::{
        export_ops::{build_local_account_export_document, mark_local_accounts_moved},
        exports::{
            finish_account_export, normalize_account_ids, AccountExportInput, AccountExportResult,
        },
        import_orchestrator::{
            pick_account_import_documents, read_import_documents, stage_returned_remote_account,
        },
        quota_refresh::prepare_preserved_remote_account_credentials,
    },
    error::{CommandError, ErrorCode, LocalPoolError},
    models::{OwnershipOperationKind, OwnershipOperationPhase, OwnershipOperationRecord},
    profiles::codex,
    remote::{
        self,
        client::{RemoteClient, RemoteClientError},
        deployment::{self, DeploymentPlan},
        RemoteTargetRecord,
    },
    state::DesktopState,
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use zenith_relay_core::accounts::{AccountAuthState, AccountExportFormat, AccountExportRequest};
use zenith_relay_core::protocol::{
    AccountSummary, Capabilities, ConfigurationPreset, ConfigurationPresetApplyInput,
    ConfigurationPresetApplyResult, ConfigurationPresetPreview, ConfigurationPresetPreviewInput,
    Feature, GatewayDiagnostic, HealthResponse, OperationalStatus, RemoteAccountLocation,
    RevealedAccountIdentity, RuntimeStateSnapshot, UsagePage, UsageQuery,
};
use zenith_relay_core::{CandidateRuntimeSnapshot, SourceProviderStats};

use super::{pool::write_configuration_preset, restart_or_rollback};

const REMOTE_TRANSFER_VALIDATION_BATCH_SIZE: usize = 5;
const ACCOUNT_TRANSFER_PROGRESS_EVENT: &str = "relay-account-transfer-progress";
const REMOTE_MISSING_ERROR: &str = "remote_missing";
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
    SetModelEnabled,
    SetModelPrice,
    SetModelReasoning,
    StartGateway,
    StopGateway,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveLocalAccountsToRemoteInput {
    pub account_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveLocalAccountsToRemoteResult {
    pub moved: usize,
    pub remote_account_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReturnRemoteAccountToLocalInput {
    pub local_account_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnRemoteAccountToLocalResult {
    pub local_account_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForceActivateRemoteAccountLocallyInput {
    pub local_account_id: String,
    pub confirm_remote_may_still_be_running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceActivateRemoteAccountLocallyResult {
    pub local_account_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountTransferProgressEvent {
    completed: usize,
    total: usize,
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_account_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportSession {
    session_id: String,
    prepared: bool,
    preview: RemoteBatchImportPreview,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportPreview {
    rows: Vec<RemoteBatchImportRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportRow {
    item_id: String,
    status: String,
    selectable: bool,
    existing: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportConfirmation {
    session_id: String,
    results: Vec<RemoteBatchImportResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportResult {
    item_id: String,
    status: String,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug)]
struct RemoteTransferConfirmationError {
    message: &'static str,
    created_account_ids: Vec<String>,
    uncertain: bool,
}

struct RemoteTransferBatch {
    account_ids: Vec<String>,
    created_account_ids: Vec<String>,
}

struct RemoteTransferBatchError {
    code: ErrorCode,
    message: String,
    created_account_ids: Vec<String>,
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
        reconcile_remote_account_locations(&state, &target, snapshot)?;
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
    reconcile_remote_account_locations(&state, &target, &snapshot)?;
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
    ensure_no_pending_ownership_operation(&state)?;
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

#[tauri::command]
pub async fn move_local_accounts_to_remote(
    input: MoveLocalAccountsToRemoteInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<MoveLocalAccountsToRemoteResult, CommandError> {
    let account_ids = normalize_account_ids(input.account_ids)?;
    let _mutation = state.setup_guard().await;
    ensure_no_pending_ownership_operation(&state)?;
    ensure_local_accounts_transferable(&state, &account_ids)?;
    emit_account_transfer_progress(&app, 0, &account_ids, "preparing");
    let Some((target, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let capabilities = client.capabilities().await.map_err(remote_error)?;
    if !capabilities.supports(Feature::AccountBatchImport) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote server does not support account batch import",
        )
        .into());
    }
    let operation = new_move_operation(&target, account_ids);
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    let remote_account_ids =
        execute_move_operation(&state, &client, &target, operation, Some(&app)).await?;

    Ok(MoveLocalAccountsToRemoteResult {
        moved: remote_account_ids.len(),
        remote_account_ids,
    })
}

#[tauri::command]
pub async fn return_remote_account_to_local(
    input: ReturnRemoteAccountToLocalInput,
    state: State<'_, DesktopState>,
) -> Result<ReturnRemoteAccountToLocalResult, CommandError> {
    let local_account_id = normalize_account_ids(vec![input.local_account_id])?
        .pop()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "account id is required"))?;
    let _mutation = state.setup_guard().await;
    ensure_no_pending_ownership_operation(&state)?;
    let remote_location = state
        .store()?
        .account(&local_account_id)
        .and_then(|account| account.remote_location.clone())
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account is not managed by a server",
            )
        })?;
    let Some((target, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    if target.server_id != remote_location.server_id {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "connect the server that owns this account before returning it",
        )
        .into());
    }
    let capabilities = client.capabilities().await.map_err(remote_error)?;
    if !capabilities.supports(Feature::AccountExport) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote server does not support account return",
        )
        .into());
    }
    let operation = new_return_operation(&target, local_account_id.clone(), remote_location);
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    execute_return_operation(&state, &client, operation).await?;
    Ok(ReturnRemoteAccountToLocalResult { local_account_id })
}

#[tauri::command]
pub async fn force_activate_remote_account_locally(
    input: ForceActivateRemoteAccountLocallyInput,
    state: State<'_, DesktopState>,
) -> Result<ForceActivateRemoteAccountLocallyResult, CommandError> {
    if !input.confirm_remote_may_still_be_running {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "explicit confirmation is required for lost-server recovery",
        )
        .into());
    }
    let local_account_id = normalize_account_ids(vec![input.local_account_id])?
        .pop()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "account id is required"))?;
    let _mutation = state.setup_guard().await;
    ensure_no_pending_ownership_operation(&state)?;
    let remote_location = state
        .store()?
        .account(&local_account_id)
        .and_then(|account| account.remote_location.clone())
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account is not managed by a server",
            )
        })?;
    // A missing management channel is the reason this explicit recovery path exists.
    if let Ok(Some((target, client))) = active_client(&state) {
        if target.server_id == remote_location.server_id
            && client.state().await.ok().is_some_and(|snapshot| {
                snapshot
                    .accounts
                    .iter()
                    .any(|account| account.id == remote_location.remote_account_id)
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "the remote account is reachable; return it through the normal operation",
            )
            .into());
        }
    }
    let operation = new_force_activation_operation(&remote_location, local_account_id.clone());
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    execute_force_activation(&state, operation).await?;
    Ok(ForceActivateRemoteAccountLocallyResult { local_account_id })
}

fn ensure_no_pending_ownership_operation(state: &DesktopState) -> Result<(), LocalPoolError> {
    if state.store()?.ownership_operation().is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "a remote account ownership operation must recover before another can start",
        ));
    }
    Ok(())
}

fn new_move_operation(
    target: &RemoteTargetRecord,
    local_account_ids: Vec<String>,
) -> OwnershipOperationRecord {
    let now = now_ms();
    OwnershipOperationRecord {
        id: format!("ownership_{}", uuid::Uuid::new_v4().simple()),
        kind: OwnershipOperationKind::MoveToRemote,
        phase: OwnershipOperationPhase::MovePrepared,
        server_id: target.server_id.clone(),
        local_account_ids,
        remote_account_ids: Vec::new(),
        created_remote_account_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

fn new_return_operation(
    target: &RemoteTargetRecord,
    local_account_id: String,
    remote_location: RemoteAccountLocation,
) -> OwnershipOperationRecord {
    let now = now_ms();
    OwnershipOperationRecord {
        id: format!("ownership_{}", uuid::Uuid::new_v4().simple()),
        kind: OwnershipOperationKind::ReturnToLocal,
        phase: OwnershipOperationPhase::ReturnPrepared,
        server_id: target.server_id.clone(),
        local_account_ids: vec![local_account_id],
        remote_account_ids: vec![remote_location.remote_account_id],
        created_remote_account_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

fn new_force_activation_operation(
    remote_location: &RemoteAccountLocation,
    local_account_id: String,
) -> OwnershipOperationRecord {
    let now = now_ms();
    OwnershipOperationRecord {
        id: format!("ownership_{}", uuid::Uuid::new_v4().simple()),
        kind: OwnershipOperationKind::ForceActivateLocal,
        phase: OwnershipOperationPhase::ForcePrepared,
        server_id: remote_location.server_id.clone(),
        local_account_ids: vec![local_account_id],
        remote_account_ids: vec![remote_location.remote_account_id.clone()],
        created_remote_account_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

async fn execute_move_operation(
    state: &DesktopState,
    client: &RemoteClient,
    target: &RemoteTargetRecord,
    mut operation: OwnershipOperationRecord,
    app: Option<&AppHandle>,
) -> Result<Vec<String>, LocalPoolError> {
    let account_ids = operation.local_account_ids.clone();
    let mut remote_account_ids = Vec::with_capacity(account_ids.len());
    let mut created_remote_account_ids = operation.created_remote_account_ids.clone();
    let mut completed = 0;
    for batch in account_ids.chunks(REMOTE_TRANSFER_VALIDATION_BATCH_SIZE) {
        if let Some(app) = app {
            emit_account_transfer_progress(app, completed, &account_ids, "transferring");
        }
        match transfer_local_account_batch(state, client, batch).await {
            Ok(transferred) => {
                remote_account_ids.extend(transferred.account_ids);
                extend_unique(
                    &mut created_remote_account_ids,
                    transferred.created_account_ids,
                );
                operation.phase = OwnershipOperationPhase::MoveRemoteApplying;
                operation.remote_account_ids = remote_account_ids.clone();
                operation.created_remote_account_ids = created_remote_account_ids.clone();
                operation.updated_at_ms = now_ms();
                state
                    .store()?
                    .replace_ownership_operation(Some(operation.clone()))?;
                for _ in batch {
                    completed += 1;
                    if let Some(app) = app {
                        emit_account_transfer_progress(
                            app,
                            completed,
                            &account_ids,
                            "transferring",
                        );
                    }
                }
            }
            Err(error) => {
                extend_unique(&mut created_remote_account_ids, error.created_account_ids);
                operation.phase = OwnershipOperationPhase::MoveRemoteApplying;
                operation.remote_account_ids = remote_account_ids;
                operation.created_remote_account_ids = created_remote_account_ids.clone();
                operation.updated_at_ms = now_ms();
                state
                    .store()?
                    .replace_ownership_operation(Some(operation))?;
                let rollback_complete =
                    delete_remote_accounts(client, &created_remote_account_ids).await;
                if rollback_complete && error.code != ErrorCode::RecoveryRequired {
                    state.store()?.replace_ownership_operation(None)?;
                    return Err(LocalPoolError::new(error.code, error.message));
                }
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{}; remote ownership recovery is required", error.message),
                ));
            }
        }
    }

    operation.phase = OwnershipOperationPhase::MoveRemoteCommitted;
    operation.remote_account_ids = remote_account_ids.clone();
    operation.created_remote_account_ids = created_remote_account_ids.clone();
    operation.updated_at_ms = now_ms();
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    let remote_locations = move_remote_locations(target, &operation)?;
    if let Some(app) = app {
        emit_account_transfer_progress(app, completed, &account_ids, "committing");
    }
    if let Err(error) =
        deactivate_transferred_local_accounts(state, &remote_locations, &operation).await
    {
        let rollback_complete = delete_remote_accounts(client, &created_remote_account_ids).await;
        if rollback_complete {
            state.store()?.replace_ownership_operation(None)?;
        }
        let message = if rollback_complete {
            format!("remote import was rolled back after local deactivation failed: {error}")
        } else {
            format!("local deactivation failed and remote recovery is incomplete: {error}")
        };
        return Err(LocalPoolError::new(ErrorCode::RecoveryRequired, message));
    }
    state.store()?.replace_ownership_operation(None)?;
    if let Some(app) = app {
        emit_account_transfer_progress(app, completed, &account_ids, "complete");
    }
    Ok(remote_account_ids)
}

async fn execute_return_operation(
    state: &DesktopState,
    client: &RemoteClient,
    mut operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let local_account_id = operation
        .local_account_ids
        .first()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "return operation has no local account",
            )
        })?;
    let remote_account_id = operation
        .remote_account_ids
        .first()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "return operation has no remote account",
            )
        })?;
    if operation.local_account_ids.len() != 1 || operation.remote_account_ids.len() != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "return operation must contain one account",
        ));
    }

    if operation.phase == OwnershipOperationPhase::ReturnLocalCommitted {
        if !local_return_is_committed(state, &local_account_id)? {
            activate_returned_local_account(state, &local_account_id, &operation).await?;
        }
        state.store()?.replace_ownership_operation(None)?;
        return Ok(());
    }

    if operation.phase == OwnershipOperationPhase::ReturnPrepared {
        let document = client
            .export_accounts(&AccountExportRequest {
                account_ids: vec![remote_account_id.clone()],
                format: AccountExportFormat::Zenith,
                description: None,
            })
            .await
            .map_err(|error| {
                LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string())
            })?;
        stage_returned_remote_account(state, &local_account_id, &document.content).await?;
        operation.phase = OwnershipOperationPhase::ReturnLocalStaged;
        operation.updated_at_ms = now_ms();
        state
            .store()?
            .replace_ownership_operation(Some(operation.clone()))?;
    }

    if operation.phase == OwnershipOperationPhase::ReturnLocalStaged {
        ensure_local_ownership_is_staged(state, &local_account_id, &operation)?;
        remove_remote_account_for_return(client, &remote_account_id).await?;
        operation.phase = OwnershipOperationPhase::ReturnRemoteRemoved;
        operation.updated_at_ms = now_ms();
        state
            .store()?
            .replace_ownership_operation(Some(operation.clone()))?;
    }

    if operation.phase == OwnershipOperationPhase::ReturnRemoteRemoved {
        activate_returned_local_account(state, &local_account_id, &operation).await?;
    }
    state.store()?.replace_ownership_operation(None)?;
    Ok(())
}

async fn execute_force_activation(
    state: &DesktopState,
    mut operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let local_account_id = operation
        .local_account_ids
        .first()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "forced recovery operation has no local account",
            )
        })?;
    if operation.local_account_ids.len() != 1 || operation.remote_account_ids.len() != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "forced recovery operation must contain one account",
        ));
    }
    if operation.phase == OwnershipOperationPhase::ForceLocalCommitted {
        if !local_return_is_committed(state, &local_account_id)? {
            ensure_local_ownership_is_staged(state, &local_account_id, &operation)?;
            activate_local_account_with_operation(state, &local_account_id, operation.clone())
                .await?;
        }
        state.store()?.replace_ownership_operation(None)?;
        return Ok(());
    }
    ensure_local_ownership_is_staged(state, &local_account_id, &operation)?;
    prepare_preserved_remote_account_credentials(state, &local_account_id).await?;
    operation.phase = OwnershipOperationPhase::ForceLocalCommitted;
    operation.updated_at_ms = now_ms();
    activate_local_account_with_operation(state, &local_account_id, operation).await?;
    state.store()?.replace_ownership_operation(None)?;
    Ok(())
}

fn ensure_local_ownership_is_staged(
    state: &DesktopState,
    local_account_id: &str,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let account = state
        .store()?
        .account(local_account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local account not found"))?;
    if account.account.enabled
        || account.account.in_pool
        || account.remote_location.as_ref().is_none_or(|location| {
            location.server_id != operation.server_id
                || operation.remote_account_ids.first() != Some(&location.remote_account_id)
        })
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account recovery credentials are not staged on the inactive local record",
        ));
    }
    Ok(())
}

async fn remove_remote_account_for_return(
    client: &RemoteClient,
    remote_account_id: &str,
) -> Result<(), LocalPoolError> {
    if !remote_account_exists(client, remote_account_id).await? {
        return Ok(());
    }
    let path = object_path("accounts", remote_account_id)
        .map_err(|error| LocalPoolError::new(error.code, error.message))?;
    match client.mutate(Method::DELETE, &path, None).await {
        Ok(_) | Err(RemoteClientError::HttpStatus(404)) => {}
        Err(error) => {
            return Err(LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                error.to_string(),
            ))
        }
    }
    if remote_account_exists(client, remote_account_id).await? {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "remote server still reports the account after deletion",
        ));
    }
    Ok(())
}

async fn remote_account_exists(
    client: &RemoteClient,
    remote_account_id: &str,
) -> Result<bool, LocalPoolError> {
    client
        .state()
        .await
        .map(|snapshot| {
            snapshot
                .accounts
                .iter()
                .any(|account| account.id == remote_account_id)
        })
        .map_err(|error| LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string()))
}

async fn activate_returned_local_account(
    state: &DesktopState,
    local_account_id: &str,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let mut committed = operation.clone();
    committed.phase = OwnershipOperationPhase::ReturnLocalCommitted;
    committed.updated_at_ms = now_ms();
    activate_local_account_with_operation(state, local_account_id, committed).await
}

async fn activate_local_account_with_operation(
    state: &DesktopState,
    local_account_id: &str,
    committed_operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let (old_accounts, old_keys, old_operation) = {
        let store = state.store()?;
        (
            store.accounts().to_vec(),
            store.keys().to_vec(),
            store.ownership_operation().cloned(),
        )
    };
    let mut accounts = old_accounts.clone();
    let account = accounts
        .iter_mut()
        .find(|account| account.account.id == local_account_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local account not found"))?;
    account.remote_location = None;
    account.account.enabled = true;
    account.account.in_pool = true;
    account.account.draining = false;
    if account.account.last_error_code.as_deref() == Some(REMOTE_MISSING_ERROR) {
        account.account.last_error_code = None;
    }
    state
        .store()?
        .replace_accounts_keys_and_ownership_operation(
            accounts,
            old_keys.clone(),
            Some(committed_operation),
        )?;
    restart_or_rollback(state, || {
        state
            .store()?
            .replace_accounts_keys_and_ownership_operation(old_accounts, old_keys, old_operation)
    })
    .await?;
    let _ = state.sync_account_quota_refresh(local_account_id, now_ms());
    Ok(())
}

fn local_return_is_committed(
    state: &DesktopState,
    local_account_id: &str,
) -> Result<bool, LocalPoolError> {
    Ok(state
        .store()?
        .account(local_account_id)
        .is_some_and(|account| {
            account.remote_location.is_none() && account.account.enabled && account.account.in_pool
        }))
}

fn move_remote_locations(
    target: &RemoteTargetRecord,
    operation: &OwnershipOperationRecord,
) -> Result<HashMap<String, RemoteAccountLocation>, LocalPoolError> {
    if operation.local_account_ids.len() != operation.remote_account_ids.len() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "remote ownership operation does not contain every account mapping",
        ));
    }
    Ok(operation
        .local_account_ids
        .iter()
        .cloned()
        .zip(
            operation
                .remote_account_ids
                .iter()
                .cloned()
                .map(|remote_account_id| RemoteAccountLocation {
                    server_id: target.server_id.clone(),
                    remote_account_id,
                }),
        )
        .collect())
}

fn extend_unique(target: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

pub(crate) async fn recover_pending_remote_ownership(
    state: &DesktopState,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let Some(operation) = state.store()?.ownership_operation().cloned() else {
        return Ok(());
    };
    if operation.kind == OwnershipOperationKind::ForceActivateLocal {
        execute_force_activation(state, operation).await?;
        return Ok(());
    }
    let Some((target, client)) = active_client(state)? else {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "reconnect the recorded server to recover account ownership",
        )
        .into());
    };
    if target.server_id != operation.server_id {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "the connected server does not own the pending account operation",
        )
        .into());
    }
    match operation.kind {
        OwnershipOperationKind::MoveToRemote => {
            if operation.phase == OwnershipOperationPhase::MoveLocalCommitted {
                if !local_move_is_committed(state, &operation)? {
                    let locations = move_remote_locations(&target, &operation)?;
                    deactivate_transferred_local_accounts(state, &locations, &operation).await?;
                }
                state.store()?.replace_ownership_operation(None)?;
                return Ok(());
            }
            ensure_move_accounts_still_present(state, &operation)?;
            let capabilities = client.capabilities().await.map_err(remote_error)?;
            if !capabilities.supports(Feature::AccountBatchImport) {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "the connected server cannot recover the pending account move",
                )
                .into());
            }
            execute_move_operation(state, &client, &target, operation, None).await?;
        }
        OwnershipOperationKind::ReturnToLocal => {
            if matches!(
                operation.phase,
                OwnershipOperationPhase::ReturnPrepared
                    | OwnershipOperationPhase::ReturnLocalStaged
            ) {
                let capabilities = client.capabilities().await.map_err(remote_error)?;
                if !capabilities.supports(Feature::AccountExport) {
                    return Err(LocalPoolError::new(
                        ErrorCode::RecoveryRequired,
                        "the connected server cannot recover the pending account return",
                    )
                    .into());
                }
            }
            execute_return_operation(state, &client, operation).await?;
        }
        OwnershipOperationKind::ForceActivateLocal => unreachable!(),
    }
    Ok(())
}

pub(crate) async fn reconcile_saved_remote_ownership(
    state: &DesktopState,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let (has_pending_operation, has_linked_accounts) = {
        let store = state.store()?;
        let Some(target) = store.remote_target() else {
            return Ok(());
        };
        (
            store.ownership_operation().is_some(),
            store.accounts().iter().any(|account| {
                account
                    .remote_location
                    .as_ref()
                    .is_some_and(|location| location.server_id == target.server_id)
            }),
        )
    };
    if has_pending_operation || !has_linked_accounts {
        return Ok(());
    }
    let Some((target, client)) = active_client(state)? else {
        return Ok(());
    };
    let snapshot = client.state().await.map_err(remote_error)?;
    reconcile_remote_account_locations(state, &target, &snapshot)?;
    Ok(())
}

fn reconcile_remote_account_locations(
    state: &DesktopState,
    target: &RemoteTargetRecord,
    snapshot: &RuntimeStateSnapshot,
) -> Result<(), LocalPoolError> {
    let remote_ids = snapshot
        .accounts
        .iter()
        .map(|account| account.id.as_str())
        .collect::<HashSet<_>>();
    let (mut accounts, keys) = {
        let store = state.store()?;
        (store.accounts().to_vec(), store.keys().to_vec())
    };
    let mut changed = false;
    for account in &mut accounts {
        let Some(location) = account
            .remote_location
            .as_ref()
            .filter(|location| location.server_id == target.server_id)
        else {
            continue;
        };
        let next_error = reconciled_remote_error(
            account.account.last_error_code.as_deref(),
            remote_ids.contains(location.remote_account_id.as_str()),
        );
        if account.account.last_error_code != next_error {
            account.account.last_error_code = next_error;
            changed = true;
        }
        if account.account.enabled || account.account.in_pool {
            account.account.enabled = false;
            account.account.in_pool = false;
            changed = true;
        }
    }
    if changed {
        state.store()?.replace_accounts_and_keys(accounts, keys)?;
    }
    Ok(())
}

fn reconciled_remote_error(current: Option<&str>, remote_exists: bool) -> Option<String> {
    if remote_exists {
        current
            .filter(|code| *code != REMOTE_MISSING_ERROR)
            .map(str::to_string)
    } else {
        Some(REMOTE_MISSING_ERROR.to_string())
    }
}

fn ensure_move_accounts_still_present(
    state: &DesktopState,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let store = state.store()?;
    if operation.local_account_ids.iter().any(|account_id| {
        store.account(account_id).is_none_or(|account| {
            account.remote_location.as_ref().is_some_and(|location| {
                location.server_id != operation.server_id
                    || !operation
                        .remote_account_ids
                        .contains(&location.remote_account_id)
            })
        })
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "a local account changed while its remote move was incomplete",
        ));
    }
    Ok(())
}

fn local_move_is_committed(
    state: &DesktopState,
    operation: &OwnershipOperationRecord,
) -> Result<bool, LocalPoolError> {
    if operation.local_account_ids.len() != operation.remote_account_ids.len() {
        return Ok(false);
    }
    let store = state.store()?;
    Ok(operation
        .local_account_ids
        .iter()
        .zip(&operation.remote_account_ids)
        .all(|(local_id, remote_id)| {
            store.account(local_id).is_some_and(|account| {
                !account.account.enabled
                    && !account.account.in_pool
                    && account.remote_location.as_ref().is_some_and(|location| {
                        location.server_id == operation.server_id
                            && location.remote_account_id == *remote_id
                    })
            })
        }))
}

fn ensure_local_accounts_transferable(
    state: &DesktopState,
    account_ids: &[String],
) -> Result<(), LocalPoolError> {
    {
        let store = state.store()?;
        for account_id in account_ids {
            let account = store.account(account_id).ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::NotFound,
                    "an account selected for transfer was not found",
                )
            })?;
            if !account_auth_can_transfer_to_remote(account.account.auth_state) {
                return Err(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "access-only accounts must sign in again before server transfer",
                ));
            }
            if account.remote_location.is_some() {
                return Err(LocalPoolError::new(
                    ErrorCode::Conflict,
                    "account is already managed by a remote server",
                ));
            }
        }
    }
    let bindings = codex::profile_bindings(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
    )?;
    if bindings.iter().any(|binding| {
        binding.active
            && (account_ids.contains(&binding.credential_id)
                || binding
                    .bound_oauth_account_id
                    .as_ref()
                    .is_some_and(|id| account_ids.contains(id)))
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "restore the direct ChatGPT profile or attach it to the remote gateway before moving this account",
        ));
    }
    Ok(())
}

fn account_auth_can_transfer_to_remote(auth_state: AccountAuthState) -> bool {
    auth_state != AccountAuthState::DegradedAccessOnly
}

fn emit_account_transfer_progress(
    app: &AppHandle,
    completed: usize,
    account_ids: &[String],
    phase: &'static str,
) {
    let _ = app.emit(
        ACCOUNT_TRANSFER_PROGRESS_EVENT,
        AccountTransferProgressEvent {
            completed,
            total: account_ids.len(),
            phase,
            current_account_id: account_ids.get(completed).cloned(),
        },
    );
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

async fn transfer_local_account_batch(
    state: &DesktopState,
    client: &RemoteClient,
    account_ids: &[String],
) -> Result<RemoteTransferBatch, RemoteTransferBatchError> {
    let document =
        build_local_account_export_document(account_ids, AccountExportFormat::Zenith, None, state)
            .map_err(|error| RemoteTransferBatchError {
                code: error.code,
                message: error.message,
                created_account_ids: Vec::new(),
            })?;
    let preview_value = client
        .mutate(
            Method::POST,
            "/accounts/import/batch/preview",
            Some(&serde_json::json!({ "content": document.content })),
        )
        .await
        .map_err(|error| RemoteTransferBatchError {
            code: ErrorCode::GatewayUnavailable,
            message: error.to_string(),
            created_account_ids: Vec::new(),
        })?;
    let preview: RemoteBatchImportSession =
        serde_json::from_value(preview_value).map_err(|_| RemoteTransferBatchError {
            code: ErrorCode::InvalidState,
            message: "remote import preview is invalid".into(),
            created_account_ids: Vec::new(),
        })?;
    validate_remote_transfer_preview(&preview, account_ids.len()).map_err(|error| {
        RemoteTransferBatchError {
            code: error.code,
            message: error.message,
            created_account_ids: Vec::new(),
        }
    })?;
    let selected_item_ids = preview
        .preview
        .rows
        .iter()
        .map(|row| row.item_id.clone())
        .collect::<Vec<_>>();
    let confirmation_value = client
        .mutate(
            Method::POST,
            "/accounts/import/batch/confirm",
            Some(&serde_json::json!({
                "sessionId": &preview.session_id,
                "selectedItemIds": selected_item_ids,
                "addToPool": true,
                "probeMetadata": true,
            })),
        )
        .await
        .map_err(|_| RemoteTransferBatchError {
            code: ErrorCode::RecoveryRequired,
            message: "remote import confirmation could not be verified".into(),
            created_account_ids: Vec::new(),
        })?;
    let confirmation: RemoteBatchImportConfirmation = serde_json::from_value(confirmation_value)
        .map_err(|_| RemoteTransferBatchError {
            code: ErrorCode::RecoveryRequired,
            message: "remote import confirmation is invalid".into(),
            created_account_ids: Vec::new(),
        })?;
    let remote_account_ids = validate_remote_transfer_confirmation(&preview, confirmation)
        .map_err(|error| RemoteTransferBatchError {
            code: if error.uncertain {
                ErrorCode::RecoveryRequired
            } else {
                ErrorCode::GatewayUnavailable
            },
            message: error.message.into(),
            created_account_ids: error.created_account_ids,
        })?;
    let created_account_ids = preview
        .preview
        .rows
        .iter()
        .zip(&remote_account_ids)
        .filter_map(|(row, account_id)| (!row.existing).then_some(account_id.clone()))
        .collect::<Vec<_>>();
    let snapshot = client
        .state()
        .await
        .map_err(|error| RemoteTransferBatchError {
            code: ErrorCode::GatewayUnavailable,
            message: error.to_string(),
            created_account_ids: created_account_ids.clone(),
        })?;
    if !remote_accounts_are_validated(&snapshot.accounts, &remote_account_ids) {
        return Err(RemoteTransferBatchError {
            code: ErrorCode::GatewayUnavailable,
            message: "remote account validation did not complete successfully".into(),
            created_account_ids,
        });
    }
    Ok(RemoteTransferBatch {
        account_ids: remote_account_ids,
        created_account_ids,
    })
}

fn validate_remote_transfer_preview(
    preview: &RemoteBatchImportSession,
    expected_accounts: usize,
) -> Result<(), CommandError> {
    if !preview.prepared
        || !valid_generated_id(&preview.session_id, "batch_")
        || preview.preview.rows.len() != expected_accounts
    {
        return Err(invalid_remote_transfer(
            "remote import preview is incomplete",
        ));
    }
    let mut seen = HashSet::new();
    if preview.preview.rows.iter().any(|row| {
        !row.selectable
            || !matches!(row.status.as_str(), "ready" | "existing")
            || !valid_generated_id(&row.item_id, "import_")
            || !seen.insert(row.item_id.as_str())
    }) {
        return Err(invalid_remote_transfer(
            "remote server rejected one or more selected accounts",
        ));
    }
    Ok(())
}

fn validate_remote_transfer_confirmation(
    preview: &RemoteBatchImportSession,
    confirmation: RemoteBatchImportConfirmation,
) -> Result<Vec<String>, RemoteTransferConfirmationError> {
    let mut complete = confirmation.session_id == preview.session_id
        && confirmation.results.len() == preview.preview.rows.len();
    let mut uncertain = !complete;
    let mut results = HashMap::with_capacity(confirmation.results.len());
    for result in confirmation.results {
        if results.insert(result.item_id.clone(), result).is_some() {
            complete = false;
            uncertain = true;
        }
    }
    let mut account_ids = Vec::with_capacity(preview.preview.rows.len());
    let mut created_account_ids = Vec::new();
    for row in &preview.preview.rows {
        let Some(result) = results.remove(&row.item_id) else {
            complete = false;
            uncertain = true;
            continue;
        };
        if result.status != "succeeded" {
            complete = false;
            continue;
        }
        let Some(account_id) = result.account_id else {
            complete = false;
            uncertain = true;
            continue;
        };
        if object_path("accounts", &account_id).is_err() {
            complete = false;
            uncertain = true;
            continue;
        }
        if !row.existing {
            created_account_ids.push(account_id.clone());
        }
        account_ids.push(account_id);
    }
    if !results.is_empty() {
        complete = false;
        uncertain = true;
    }
    if !complete || account_ids.len() != preview.preview.rows.len() {
        return Err(RemoteTransferConfirmationError {
            message: "remote server did not confirm every selected account",
            created_account_ids,
            uncertain,
        });
    }
    Ok(account_ids)
}

fn remote_accounts_are_validated(accounts: &[AccountSummary], expected_ids: &[String]) -> bool {
    expected_ids.iter().all(|account_id| {
        accounts
            .iter()
            .find(|account| account.id == *account_id)
            .is_some_and(|account| {
                account.enabled
                    && account.in_pool
                    && !account.draining
                    && account.secret_available
                    && account.proxy_available
                    && account.auth_state == AccountAuthState::Active
                    && !account.models.is_empty()
                    && account.quota.updated_at_ms.is_some()
                    && account.quota.error.is_none()
                    && !matches!(
                        account.last_error_code.as_deref(),
                        Some("metadata_refresh_failed" | "runtime_rebuild_failed")
                    )
                    && matches!(
                        account.operational_status,
                        OperationalStatus::Rotation | OperationalStatus::QuotaWait
                    )
            })
    })
}

async fn deactivate_transferred_local_accounts(
    state: &DesktopState,
    remote_locations: &HashMap<String, RemoteAccountLocation>,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let (old_accounts, old_keys, old_operation) = {
        let store = state.store()?;
        (
            store.accounts().to_vec(),
            store.keys().to_vec(),
            store.ownership_operation().cloned(),
        )
    };
    let mut accounts = old_accounts.clone();
    mark_local_accounts_moved(&mut accounts, remote_locations)?;
    let mut committed = operation.clone();
    committed.phase = OwnershipOperationPhase::MoveLocalCommitted;
    committed.updated_at_ms = now_ms();
    state
        .store()?
        .replace_accounts_keys_and_ownership_operation(
            accounts,
            old_keys.clone(),
            Some(committed),
        )?;
    restart_or_rollback(state, || {
        state
            .store()?
            .replace_accounts_keys_and_ownership_operation(old_accounts, old_keys, old_operation)
    })
    .await?;
    for account_id in remote_locations.keys() {
        let _ = state.sync_account_quota_refresh(account_id, now_ms());
    }
    Ok(())
}

async fn delete_remote_accounts(client: &RemoteClient, account_ids: &[String]) -> bool {
    let mut complete = true;
    for account_id in account_ids {
        let Ok(path) = object_path("accounts", account_id) else {
            complete = false;
            continue;
        };
        if client.mutate(Method::DELETE, &path, None).await.is_err() {
            complete = false;
        }
    }
    complete
}

fn valid_generated_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn invalid_remote_transfer(message: &str) -> CommandError {
    LocalPoolError::new(ErrorCode::InvalidState, message).into()
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
        RemoteServerAction::SetModelEnabled => (Method::POST, "/models/rules".to_string(), true),
        RemoteServerAction::SetModelPrice => (Method::POST, "/models/prices".to_string(), true),
        RemoteServerAction::SetModelReasoning => {
            (Method::POST, "/models/reasoning".to_string(), true)
        }
        RemoteServerAction::StartGateway => (Method::POST, "/gateway/start".to_string(), false),
        RemoteServerAction::StopGateway => (Method::POST, "/gateway/stop".to_string(), false),
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
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
    fn access_only_account_cannot_start_server_transfer() {
        assert!(account_auth_can_transfer_to_remote(
            AccountAuthState::Active
        ));
        assert!(!account_auth_can_transfer_to_remote(
            AccountAuthState::DegradedAccessOnly
        ));
    }

    #[test]
    fn transfer_confirmation_preserves_preview_order() {
        let preview = preview(false, true);
        let confirmation = RemoteBatchImportConfirmation {
            session_id: preview.session_id.clone(),
            results: vec![
                result("import_22222222222222222222222222222222", "remote-two"),
                result("import_11111111111111111111111111111111", "remote-one"),
            ],
        };

        assert_eq!(
            validate_remote_transfer_confirmation(&preview, confirmation).unwrap(),
            vec!["remote-one", "remote-two"]
        );
    }

    #[test]
    fn partial_transfer_reports_only_new_accounts_for_rollback() {
        let preview = preview(false, true);
        let confirmation = RemoteBatchImportConfirmation {
            session_id: preview.session_id.clone(),
            results: vec![
                result("import_11111111111111111111111111111111", "remote-new"),
                RemoteBatchImportResult {
                    item_id: "import_22222222222222222222222222222222".into(),
                    status: "failed".into(),
                    account_id: None,
                },
            ],
        };

        let error = validate_remote_transfer_confirmation(&preview, confirmation).unwrap_err();
        assert_eq!(error.created_account_ids, vec!["remote-new"]);
        assert!(!error.uncertain);
    }

    #[test]
    fn local_routing_waits_for_complete_remote_account_validation() {
        let mut account = validated_account("remote-one");
        assert!(remote_accounts_are_validated(
            &[account.clone()],
            &["remote-one".into()]
        ));

        account.last_error_code = Some("runtime_rebuild_failed".into());
        assert!(!remote_accounts_are_validated(
            &[account],
            &["remote-one".into()]
        ));
        assert!(!remote_accounts_are_validated(&[], &["remote-one".into()]));
    }

    #[test]
    fn remote_reconciliation_is_fail_closed_and_clears_only_its_own_error() {
        assert_eq!(
            reconciled_remote_error(None, false).as_deref(),
            Some(REMOTE_MISSING_ERROR)
        );
        assert_eq!(
            reconciled_remote_error(Some(REMOTE_MISSING_ERROR), true),
            None
        );
        assert_eq!(
            reconciled_remote_error(Some("token_invalidated"), true).as_deref(),
            Some("token_invalidated")
        );
    }

    #[test]
    fn forced_local_recovery_is_a_valid_persisted_ownership_operation() {
        let operation = new_force_activation_operation(
            &RemoteAccountLocation {
                server_id: "server-one".into(),
                remote_account_id: "account-remote".into(),
            },
            "account-local".into(),
        );

        assert!(operation.validate().is_ok());
        assert_eq!(operation.kind, OwnershipOperationKind::ForceActivateLocal);
        assert_eq!(operation.phase, OwnershipOperationPhase::ForcePrepared);
    }

    fn preview(first_existing: bool, second_existing: bool) -> RemoteBatchImportSession {
        RemoteBatchImportSession {
            session_id: "batch_00000000000000000000000000000000".into(),
            prepared: true,
            preview: RemoteBatchImportPreview {
                rows: vec![
                    row("import_11111111111111111111111111111111", first_existing),
                    row("import_22222222222222222222222222222222", second_existing),
                ],
            },
        }
    }

    fn row(item_id: &str, existing: bool) -> RemoteBatchImportRow {
        RemoteBatchImportRow {
            item_id: item_id.into(),
            status: if existing { "existing" } else { "ready" }.into(),
            selectable: true,
            existing,
        }
    }

    fn result(item_id: &str, account_id: &str) -> RemoteBatchImportResult {
        RemoteBatchImportResult {
            item_id: item_id.into(),
            status: "succeeded".into(),
            account_id: Some(account_id.into()),
        }
    }

    fn validated_account(account_id: &str) -> AccountSummary {
        serde_json::from_value(serde_json::json!({
            "id": account_id,
            "label": "Synthetic account",
            "identityHint": "sy••••ic",
            "enabled": true,
            "inPool": true,
            "draining": false,
            "operationalStatus": "rotation",
            "authState": { "state": "active" },
            "health": "healthy",
            "models": ["gpt-test"],
            "allowedModels": [],
            "excludedModels": [],
            "priority": 0,
            "weight": 1,
            "apiEquivalent": { "microUsd": 0, "pricedTokens": 0, "unpricedTokens": 0 },
            "subscription": {
                "planType": "plus",
                "activeUntilMs": null,
                "status": "active",
                "updatedAtMs": 1
            },
            "quota": {
                "primary": null,
                "secondary": null,
                "supplemental": [],
                "limitReached": false,
                "resetCreditsAvailable": null,
                "updatedAtMs": 1,
                "error": null
            },
            "quotaRefreshStatus": "updated",
            "secretAvailable": true,
            "proxyMode": "direct",
            "proxyAvailable": true,
            "lastErrorCode": null
        }))
        .unwrap()
    }
}

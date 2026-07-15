use crate::local_pool::{
    accounts::exports::{
        finish_account_export, normalize_account_ids, AccountExportInput, AccountExportResult,
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
use tauri::{AppHandle, State};
use zenith_relay_core::accounts::AccountExportRequest;
use zenith_relay_core::protocol::{
    Capabilities, GatewayDiagnostic, HealthResponse, RevealedAccountIdentity, RuntimeStateSnapshot,
    UsagePage, UsageQuery,
};

use super::accounts::{pick_account_import_documents, read_import_documents};

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
    RefreshPoolQuotas,
    SetModelEnabled,
    CreateKey,
    UpdateKey { id: String },
    DeleteKey { id: String },
    RotateKey { id: String },
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

#[tauri::command]
pub async fn connect_remote_server(
    input: ConnectRemoteServerInput,
    state: State<'_, DesktopState>,
) -> Result<RemoteConnectionState, CommandError> {
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
    let previous = remote::load_active(&state.root)?;
    if previous.as_ref().is_some_and(|record| {
        record.origin == client.origin()
            && record.identity_fingerprint != negotiated.identity_fingerprint
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
    if let Err(error) = remote::save_active(&state.root, Some(target.clone())) {
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
    let Some((_, client)) = active_client(&state)? else {
        return Ok(None);
    };
    client.state().await.map(Some).map_err(remote_error)
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
        })
        .await
        .map_err(remote_error)?;
    finish_account_export(document, input.destination, &app, &state)
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
    remote::save_active(&state.root, Some(target.clone()))?;
    Ok(Some(RemoteConnectionState {
        target,
        health,
        capabilities,
    }))
}

#[tauri::command]
pub fn disconnect_remote_server(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let Some(target) = remote::load_active(&state.root)? else {
        return Ok(());
    };
    let token = remote::load_token(&target)?;
    remote::delete_token(&target)?;
    if let Err(error) = remote::save_active(&state.root, None) {
        if let Some(token) = token {
            let _ = remote::save_token(&target, &token);
        }
        return Err(error.into());
    }
    Ok(())
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
pub async fn execute_remote_server_action(
    input: ExecuteRemoteServerActionInput,
    state: State<'_, DesktopState>,
) -> Result<serde_json::Value, CommandError> {
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

fn active_client(
    state: &DesktopState,
) -> Result<Option<(RemoteTargetRecord, RemoteClient)>, CommandError> {
    let Some(target) = remote::load_active(&state.root)? else {
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
    format!("remote:{:x}", Sha256::digest(origin.as_bytes()))
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
        RemoteServerAction::RefreshPoolQuotas => {
            (Method::POST, "/pool/quota/refresh".to_string(), false)
        }
        RemoteServerAction::SetModelEnabled => (Method::POST, "/models/rules".to_string(), true),
        RemoteServerAction::CreateKey => (Method::POST, "/keys".to_string(), true),
        RemoteServerAction::UpdateKey { id } => (Method::PATCH, object_path("keys", id)?, true),
        RemoteServerAction::DeleteKey { id } => (Method::DELETE, object_path("keys", id)?, false),
        RemoteServerAction::RotateKey { id } => (
            Method::POST,
            format!("{}/rotate", object_path("keys", id)?),
            false,
        ),
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

fn remote_error(error: impl std::fmt::Display) -> CommandError {
    LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string()).into()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

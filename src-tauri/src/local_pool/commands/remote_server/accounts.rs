use super::session::{active_client, remote_error};
use super::{
    ForceActivateRemoteAccountLocallyInput, ForceActivateRemoteAccountLocallyResult,
    MoveLocalAccountsToRemoteInput, MoveLocalAccountsToRemoteResult, PrepareRemoteDeploymentInput,
    ReturnRemoteAccountToLocalInput, ReturnRemoteAccountToLocalResult,
};
use crate::local_pool::{
    accounts::{
        exports::{
            finish_account_export, normalize_account_ids, normalize_one_account_id,
            AccountExportInput, AccountExportResult,
        },
        import_orchestrator::{pick_account_import_documents, read_import_documents},
    },
    error::{CommandError, ErrorCode, LocalPoolError},
    remote::deployment::{self, DeploymentPlan},
    state::DesktopState,
};
use reqwest::Method;
use tauri::{AppHandle, State};
use zenith_relay_core::accounts::AccountExportRequest;
use zenith_relay_core::protocol::RevealedAccountIdentity;

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
    let account_id = normalize_one_account_id(account_id)?;
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
    super::ownership::move_local_accounts_to_remote(input, app, state).await
}

#[tauri::command]
pub async fn return_remote_account_to_local(
    input: ReturnRemoteAccountToLocalInput,
    state: State<'_, DesktopState>,
) -> Result<ReturnRemoteAccountToLocalResult, CommandError> {
    super::ownership::return_remote_account_to_local(input, state).await
}

#[tauri::command]
pub async fn force_activate_remote_account_locally(
    input: ForceActivateRemoteAccountLocallyInput,
    state: State<'_, DesktopState>,
) -> Result<ForceActivateRemoteAccountLocallyResult, CommandError> {
    super::ownership::force_activate_remote_account_locally(input, state).await
}

use super::quota_refresh::ConfirmAccountImportResponse;
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::import_session::{ImportSession, ImportSessionStore};
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::sync_records_or_rollback;
use crate::local_pool::error::CommandError;
use crate::local_pool::models::ProviderSourceRecord;
use crate::local_pool::state::DesktopState;
use crate::local_pool::store::secret_store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tauri::{AppHandle, State};
use uuid::Uuid;
use zenith_relay_core::accounts::{
    ImportAuthMode, ImportPreview, ParsedImportItem, MAX_IMPORT_ITEMS,
};
use zenith_relay_core::{
    discover_source_models_and_protocol_bindings, ApiModelPriceOverride, ProviderSource,
    SourceProtocolBinding, WireApi,
};

mod account_import;
mod account_lookup;
mod account_policy;
mod claims;
mod confirmation;
mod credential_material;
mod current_profile;
mod documents;
mod errors;
mod identity;
mod persistence;
mod policy;
mod prepared_items;
mod preview;
mod refresh_state;
mod sources;

pub(crate) use account_import::stage_returned_remote_account;
use account_import::{hinted_import_proxy, import_account_item};
pub(in crate::local_pool::accounts) use account_lookup::{
    existing_identity_index, find_existing_account,
};
pub(in crate::local_pool::accounts) use account_policy::{
    apply_account_patch, validate_account_record,
};
pub(super) use claims::{imported_identity, parse_subscription_timestamp_ms};
pub(super) use confirmation::confirm_local_account_import_inner;
#[cfg(test)]
pub(super) use credential_material::lookup_import_account_id;
pub(in crate::local_pool::accounts) use credential_material::{
    build_import_credential_material, ImportedCredentialMaterial,
};
#[cfg(test)]
pub(super) use current_profile::{
    current_codex_import_documents, is_usable_current_chatgpt_profile,
};
use current_profile::{current_profile_available, current_profile_documents};
pub(super) use documents::normalize_import_input;
pub use documents::StartAccountImportInput;
pub(crate) use documents::{pick_account_import_documents, read_import_documents};
pub(in crate::local_pool::accounts) use errors::{
    credential_item_error, credential_local_error, import_item_command_error, import_session_error,
    model_failure_code, model_item_error, proxy_item_error, ImportItemError, ItemResult,
};
#[cfg(test)]
pub(super) use identity::normalized_profile_account_id;
pub(super) use identity::{
    account_id_from_check_response, masked_account_identity, provider_identity_key,
    timestamp_from_ms,
};
pub(in crate::local_pool::accounts) use persistence::persist_imported_account;
pub(super) use policy::{
    account_auth_mode, account_model_state_is_valid, ensure_account_import_item,
    merge_existing_account, normalize_models, normalize_selected_item_ids,
    preserve_newer_account_state, should_probe_import_quota, validate_label,
};
use prepared_items::{parsed_item_value, parsed_item_value_from_material};
use preview::{prepare_import_preview, preview_account_import_documents};
pub(in crate::local_pool::accounts) use refresh_state::{
    apply_model_discovery, apply_model_discovery_failure, apply_quota_outcome,
    apply_quota_outcome_with_transitions,
};

pub(super) use sources::*;

type CommandResult<T> = std::result::Result<T, CommandError>;

pub(super) const MAX_ACCOUNT_LABEL_BYTES: usize = 128;

pub(super) const MAX_MODELS: usize = 4_096;

pub(super) const DEFAULT_OPENAI_SOURCE_URL: &str = "https://api.openai.com/v1";

pub(super) const CODEX_ACCOUNT_CHECK_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/wham/accounts/check";

pub(super) const MAX_ACCOUNT_PROFILE_RESPONSE_BYTES: usize = 256 * 1024;

pub(super) const ACCOUNT_IMPORT_PROGRESS_EVENT: &str = "relay-account-import-progress";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareAccountImportInput {
    pub(super) session_id: String,
    #[serde(default)]
    pub(super) probe_quota: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmAccountImportInput {
    pub(super) session_id: String,
    pub(super) selected_item_ids: Vec<String>,
    #[serde(default)]
    pub(super) add_to_pool: bool,
    #[serde(default = "default_true")]
    pub(super) discover_models: bool,
    #[serde(default)]
    pub(super) probe_quota: bool,
    #[serde(default)]
    pub(super) models: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionResponse {
    pub session_id: String,
    pub created_at_ms: u64,
    pub prepared: bool,
    pub preview: ImportPreview,
}

impl From<ImportSession> for ImportSessionResponse {
    fn from(session: ImportSession) -> Self {
        Self {
            session_id: session.session_id,
            created_at_ms: session.created_at_ms,
            prepared: session.prepared,
            preview: session.preview,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportItemStatus {
    Succeeded,
    Failed,
}

#[derive(Clone)]
pub(super) struct ImportRowContext {
    pub(super) label: String,
    pub(super) auth_mode: ImportAuthMode,
    pub(super) selectable: bool,
    pub(super) plan: Option<String>,
    pub(super) subscription_active_until_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountImportProgressEvent {
    pub(super) session_id: String,
    pub(super) completed: usize,
    pub(super) total: usize,
    pub(super) succeeded: usize,
    pub(super) failed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) current_label: Option<String>,
}

#[tauri::command]
pub async fn start_local_account_import(
    input: StartAccountImportInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ImportSessionResponse> {
    let _mutation = state.setup_guard().await;
    let (content, source_file) = normalize_import_input(input)?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(&state, &credentials)?;
    let session = ImportSessionStore::new(state.transient_root(), NativeSecretBackend)
        .start(
            &content,
            source_file.as_deref(),
            &existing.keys().cloned().collect::<Vec<_>>(),
        )
        .map_err(import_session_error)?;
    Ok(session.into())
}

#[tauri::command]
pub async fn preview_local_account_import_files(
    paths: Option<Vec<PathBuf>>,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<ImportSessionResponse>> {
    let documents = match paths {
        Some(paths) => Some(read_import_documents(paths)?),
        None => pick_account_import_documents(&app)?,
    };
    let Some(documents) = documents else {
        return Ok(None);
    };
    preview_account_import_documents(documents, &state)
        .await
        .map(Some)
}

#[tauri::command]
pub async fn preview_current_codex_account_import(
    state: State<'_, DesktopState>,
) -> CommandResult<ImportSessionResponse> {
    let documents = current_profile_documents(&state)?;
    preview_account_import_documents(documents, &state).await
}

#[tauri::command]
pub async fn current_chatgpt_profile_available(
    state: State<'_, DesktopState>,
) -> CommandResult<bool> {
    current_profile_available(&state).map_err(Into::into)
}

#[tauri::command]
pub async fn resume_local_account_import(
    session_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<ImportSessionResponse> {
    let _mutation = state.setup_guard().await;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(&state, &credentials)?;
    let session = ImportSessionStore::new(state.transient_root(), NativeSecretBackend)
        .resume(&session_id, &existing.keys().cloned().collect::<Vec<_>>())
        .map_err(import_session_error)?;
    Ok(session.into())
}

#[tauri::command]
pub async fn prepare_local_account_import(
    input: PrepareAccountImportInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ImportSessionResponse> {
    let _mutation = state.setup_guard().await;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(&state, &credentials)?;
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .resume(
            &input.session_id,
            &existing.keys().cloned().collect::<Vec<_>>(),
        )
        .map_err(import_session_error)?;
    let probe_quota = should_probe_import_quota(input.probe_quota, session.preview.rows.len());
    let (content, preview) =
        prepare_import_preview(&state, &credentials, session, probe_quota).await?;
    let session = sessions
        .prepare(
            &input.session_id,
            content.as_deref(),
            preview,
            &existing.keys().cloned().collect::<Vec<_>>(),
        )
        .map_err(import_session_error)?;
    Ok(session.into())
}

#[tauri::command]
pub async fn cancel_local_account_import(
    session_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    ImportSessionStore::new(state.transient_root(), NativeSecretBackend)
        .cancel(&session_id)
        .map_err(import_session_error)?;
    Ok(())
}

#[tauri::command]
pub async fn confirm_local_account_import(
    input: ConfirmAccountImportInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<ConfirmAccountImportResponse> {
    let _mutation = state.setup_guard().await;
    confirm_local_account_import_inner(input, &state, Some(&app)).await
}

pub(super) fn default_true() -> bool {
    true
}

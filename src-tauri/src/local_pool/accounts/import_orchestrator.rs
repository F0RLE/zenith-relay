use super::quota_refresh::ConfirmAccountImportResponse;
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::import_session::{ImportSession, ImportSessionStore};
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::error::CommandError;
use crate::local_pool::state::DesktopState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tauri::{AppHandle, State};
use zenith_relay_core::accounts::{ImportAuthMode, ImportPreview, MAX_IMPORT_ITEMS};

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
pub(super) use identity::account_id_from_check_response;
#[cfg(test)]
pub(super) use identity::normalized_profile_account_id;
pub(super) use identity::{masked_account_identity, provider_identity_key, timestamp_from_ms};
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

fn record_import_command_result<T>(
    stage: &str,
    started: Instant,
    result: CommandResult<T>,
) -> CommandResult<T> {
    match result {
        Ok(value) => {
            crate::diagnostics::record_operation(
                "account-import",
                stage,
                &[("duration_ms", started.elapsed().as_millis().to_string())],
            );
            Ok(value)
        }
        Err(error) => {
            crate::diagnostics::record_error(
                "account-import",
                Some(&format!("{:?}", error.code)),
                &error.message,
                &[("duration_ms", started.elapsed().as_millis().to_string())],
            );
            Err(error)
        }
    }
}

pub(super) const MAX_ACCOUNT_LABEL_BYTES: usize = 128;

pub(super) const MAX_MODELS: usize = 4_096;

pub(super) const DEFAULT_OPENAI_SOURCE_URL: &str = "https://api.openai.com/v1";

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
    let started = Instant::now();
    crate::diagnostics::breadcrumb("account-import", "start", &[]);
    let result = async {
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
        Ok::<ImportSessionResponse, CommandError>(session.into())
    }
    .await;
    record_import_command_result("start_completed", started, result)
}

#[tauri::command]
pub async fn preview_local_account_import_files(
    paths: Option<Vec<PathBuf>>,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<ImportSessionResponse>> {
    let started = Instant::now();
    crate::diagnostics::breadcrumb("account-import", "preview_files", &[]);
    let result = async {
        let documents = match paths {
            Some(paths) => Some(read_import_documents(paths)?),
            None => pick_account_import_documents(&app)?,
        };
        let Some(documents) = documents else {
            return Ok::<Option<ImportSessionResponse>, CommandError>(None);
        };
        preview_account_import_documents(documents, &state)
            .await
            .map(Some)
    }
    .await;
    record_import_command_result("preview_files_completed", started, result)
}

#[tauri::command]
pub async fn preview_current_codex_account_import(
    state: State<'_, DesktopState>,
) -> CommandResult<ImportSessionResponse> {
    let started = Instant::now();
    crate::diagnostics::breadcrumb("account-import", "preview_current_profile", &[]);
    let result = async {
        let documents = current_profile_documents(&state)?;
        preview_account_import_documents(documents, &state).await
    }
    .await;
    record_import_command_result("preview_current_completed", started, result)
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
    crate::diagnostics::breadcrumb(
        "account-import",
        "resume",
        &[("session", crate::diagnostics::hash_identifier(&session_id))],
    );
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
    let started = Instant::now();
    let session_hash = crate::diagnostics::hash_identifier(&input.session_id);
    crate::diagnostics::breadcrumb(
        "account-import",
        "prepare",
        &[("session", session_hash.clone())],
    );
    let result = async {
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let existing = existing_identity_index(&state, &credentials)?;
        let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
        let session = sessions
            .resume(
                &input.session_id,
                &existing.keys().cloned().collect::<Vec<_>>(),
            )
            .map_err(import_session_error)?;
        let candidate_count = session.preview.rows.len();
        let probe_quota = should_probe_import_quota(input.probe_quota, candidate_count);
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
        Ok::<_, CommandError>((session, candidate_count))
    }
    .await;
    match result {
        Ok((session, candidate_count)) => {
            crate::diagnostics::record_operation(
                "account-import",
                "prepare_completed",
                &[
                    ("session", session_hash),
                    ("candidates", candidate_count.to_string()),
                    ("duration_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            Ok(session.into())
        }
        Err(error) => {
            crate::diagnostics::record_error(
                "account-import",
                Some(&format!("{:?}", error.code)),
                &error.message,
                &[
                    ("session", session_hash),
                    ("duration_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn cancel_local_account_import(
    session_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    crate::diagnostics::breadcrumb(
        "account-import",
        "cancel",
        &[("session", crate::diagnostics::hash_identifier(&session_id))],
    );
    ImportSessionStore::new(state.transient_root(), NativeSecretBackend)
        .cancel(&session_id)
        .map_err(import_session_error)?;
    Ok(())
}

#[tauri::command]
#[inline(never)]
pub async fn confirm_local_account_import(
    input: ConfirmAccountImportInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<ConfirmAccountImportResponse> {
    // Keep the command future itself small.  On Windows the optimized Tauri
    // dispatcher polls command futures on a bounded stack; embedding the
    // complete import/serialization workflow here can overflow that stack
    // before the first breadcrumb is written.  Boxing the implementation
    // makes the expensive state machine heap-backed while preserving the
    // typed IPC contract.
    Box::pin(confirm_local_account_import_impl(input, app, state)).await
}

#[inline(never)]
async fn confirm_local_account_import_impl(
    input: ConfirmAccountImportInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<ConfirmAccountImportResponse> {
    let session_hash = crate::diagnostics::hash_identifier(&input.session_id);
    let selected_count = input.selected_item_ids.len();
    let add_to_pool = input.add_to_pool;
    crate::diagnostics::breadcrumb(
        "account-import",
        "confirm_payload_validated",
        &[
            ("session", session_hash.clone()),
            ("selected_count", selected_count.to_string()),
            ("add_to_pool", add_to_pool.to_string()),
        ],
    );
    let _mutation = state.setup_guard().await;
    let started = Instant::now();
    crate::diagnostics::breadcrumb(
        "account-import",
        "confirm_lock_acquired",
        &[("session", session_hash.clone())],
    );
    crate::diagnostics::breadcrumb(
        "account-import",
        "confirm_started",
        &[
            ("session", session_hash.clone()),
            ("selected_count", selected_count.to_string()),
            ("add_to_pool", add_to_pool.to_string()),
        ],
    );
    let response = match confirm_local_account_import_inner(input, &state, Some(&app)).await {
        Ok(response) => response,
        Err(error) => {
            crate::diagnostics::record_error(
                "account-import",
                Some(&format!("{:?}", error.code)),
                &error.message,
                &[
                    ("session", session_hash),
                    ("selected_count", selected_count.to_string()),
                    ("add_to_pool", add_to_pool.to_string()),
                    ("duration_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            return Err(error);
        }
    };
    let model_refresh_account_ids = if add_to_pool {
        response
            .results
            .iter()
            .filter_map(|result| {
                result
                    .account
                    .as_ref()
                    .filter(|account| account.account.in_pool)
                    .map(|account| account.account.id.clone())
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let succeeded = response
        .results
        .iter()
        .filter(|result| result.status == ImportItemStatus::Succeeded)
        .count();
    let failed = response.results.len().saturating_sub(succeeded);
    crate::diagnostics::record_operation(
        "account-import",
        "confirm_completed",
        &[
            ("session", session_hash),
            ("selected_count", selected_count.to_string()),
            ("succeeded", succeeded.to_string()),
            ("failed", failed.to_string()),
            ("add_to_pool", add_to_pool.to_string()),
            ("duration_ms", started.elapsed().as_millis().to_string()),
        ],
    );
    drop(_mutation);
    crate::local_pool::background::refresh_account_models_in_background(
        app,
        model_refresh_account_ids,
    );
    Ok(response)
}

pub(super) fn default_true() -> bool {
    true
}

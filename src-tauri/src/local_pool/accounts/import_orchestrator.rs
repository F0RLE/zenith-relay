use super::mutations::{
    current_account_records, repair_gateway_after_item_restore, restore_credential_item,
    UpdateAccountInput,
};
use super::quota_refresh::{
    account_bearer_authorization, AccountQuotaOutcome, ConfirmAccountImportResponse,
    ImportItemResult, QUOTA_COMMAND_TIMEOUT_OVERHEAD, TOKEN_REFRESH_SKEW_MS,
};
use crate::local_pool::accounts::credentials::{
    CredentialError, CredentialErrorCode, CredentialStore, StoredCodexCredentials,
};
use crate::local_pool::accounts::import_session::{
    ImportSession, ImportSessionError, ImportSessionErrorCode, ImportSessionStore,
};
use crate::local_pool::accounts::proxy::{
    common_proxy_config, effective_proxy_config, ensure_account_proxy,
};
use crate::local_pool::accounts::quota_service::{apply_quota_failure, apply_quota_success};
use crate::local_pool::accounts::{collect_limited, oauth::CodexOAuthClient, LimitedBodyError};
use crate::local_pool::accounts::{records, NativeSecretBackend};
use crate::local_pool::commands::{
    current_time_ms, sync_accounts_or_rollback, sync_records_or_rollback,
};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use crate::local_pool::store::secret_store;
use crate::platform::default_codex_home;
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use url::Url;
use uuid::Uuid;
use zenith_relay_core::accounts::{
    combine_import_documents, parse_import, ImportAuthMode, ImportIssue, ImportIssueCode,
    ImportPreview, ImportPreviewStatus, ImportQuotaStatus, ParsedImport, ParsedImportItem,
    MAX_IMPORT_BYTES, MAX_IMPORT_ITEMS,
};
use zenith_relay_core::accounts::{
    AccountAuthState, AccountHealthState, MAX_PURCHASE_COST_MICRO_USD,
};
use zenith_relay_core::providers::chatgpt::{
    AgentIdentityCredential, CodexModelsClient, CodexQuotaClient, ModelDiscoveryFailure,
    ModelDiscoveryFailureCode, QuotaRefreshOutcome,
};
use zenith_relay_core::quota::QuotaRefreshFailure;
use zenith_relay_core::{
    discover_source_models_and_protocol_bindings, normalize_error_code, ApiModelPriceOverride,
    ProviderSource, ProxyConfig, SourceProtocolBinding, WireApi,
};

mod claims;
mod identity;
mod policy;
mod sources;

pub(super) use claims::{imported_identity, parse_subscription_timestamp_ms};
#[cfg(test)]
pub(super) use identity::normalized_profile_account_id;
pub(super) use identity::{
    account_id_from_check_response, masked_account_identity, provider_identity_key,
    timestamp_from_ms,
};
pub(super) use policy::{
    account_auth_mode, account_model_state_is_valid, ensure_account_import_item,
    merge_existing_account, normalize_models, normalize_selected_item_ids,
    preserve_newer_account_state, should_probe_import_quota, validate_label,
};

pub(super) use sources::*;

type CommandResult<T> = std::result::Result<T, CommandError>;
type ItemResult<T> = std::result::Result<T, ImportItemError>;

pub(super) const MAX_ACCOUNT_LABEL_BYTES: usize = 128;

pub(super) const MAX_MODELS: usize = 4_096;

pub(super) const DEFAULT_OPENAI_SOURCE_URL: &str = "https://api.openai.com/v1";

pub(super) const CODEX_ACCOUNT_CHECK_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/wham/accounts/check";

pub(super) const MAX_ACCOUNT_PROFILE_RESPONSE_BYTES: usize = 256 * 1024;

pub(super) const ACCOUNT_IMPORT_PROGRESS_EVENT: &str = "relay-account-import-progress";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartAccountImportInput {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) documents: Vec<String>,
    #[serde(default)]
    pub(super) source_file: Option<String>,
}

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemError {
    pub code: String,
    pub message: String,
}

impl ImportItemError {
    pub(super) fn new(code: &str, message: &str) -> Self {
        Self {
            code: normalize_error_code(code).unwrap_or_else(|| "operation_failed".to_string()),
            message: message.to_string(),
        }
    }

    pub(super) fn recovery(message: &str) -> Self {
        Self::new("recovery_required", message)
    }
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

pub(super) struct ImportedCredentialMaterial {
    pub(super) access_token: String,
    pub(super) agent_identity: Option<AgentIdentityCredential>,
    pub(super) refresh_token: Option<String>,
    pub(super) id_token: Option<String>,
    pub(super) expires_at_ms: Option<u64>,
    pub(super) email: Option<String>,
    pub(super) provider_account_id: Option<String>,
    pub(super) provider_user_id: Option<String>,
    pub(super) organization_id: Option<String>,
    pub(super) plan_type: Option<String>,
    pub(super) subscription_active_until_ms: Option<u64>,
    pub(super) account_is_fedramp: bool,
}

impl ImportedCredentialMaterial {
    pub(super) fn authorization(&self, now_ms: u64) -> ItemResult<HeaderValue> {
        if let Some(agent) = self.agent_identity.as_ref() {
            return agent.authorization(now_ms).map_err(|_| {
                ImportItemError::new(
                    "agent_identity_invalid",
                    "Agent Identity credential is invalid",
                )
            });
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.access_token))
            .map_err(|_| {
                ImportItemError::new("access_token_rejected", "imported access token is invalid")
            })?;
        authorization.set_sensitive(true);
        Ok(authorization)
    }

    pub(super) fn subscription_authorization(&self) -> ItemResult<Option<HeaderValue>> {
        if self.access_token.is_empty() {
            return Ok(None);
        }
        account_bearer_authorization(&self.access_token)
            .map(Some)
            .map_err(|_| {
                ImportItemError::new("access_token_rejected", "imported access token is invalid")
            })
    }

    pub(super) fn into_stored(
        self,
        local_account_id: &str,
        issued_at_ms: u64,
        generation: u64,
    ) -> ItemResult<StoredCodexCredentials> {
        if self.access_token.is_empty() {
            let agent_identity = self.agent_identity.ok_or_else(|| {
                ImportItemError::new(
                    "access_token_missing",
                    "ChatGPT account import has no authorization method",
                )
            })?;
            return StoredCodexCredentials::new_agent_identity(
                local_account_id,
                agent_identity,
                issued_at_ms,
                generation,
                self.email,
                self.provider_account_id,
                self.provider_user_id,
                self.organization_id,
                self.plan_type,
                self.account_is_fedramp,
            )
            .map_err(credential_item_error);
        }
        let agent_identity = self.agent_identity;
        let mut stored = StoredCodexCredentials::new(
            local_account_id,
            self.access_token,
            self.refresh_token,
            self.id_token,
            self.expires_at_ms,
            issued_at_ms,
            generation,
            self.email,
            self.provider_account_id,
            self.provider_user_id,
            self.organization_id,
            self.plan_type,
            self.account_is_fedramp,
        )
        .map_err(credential_item_error)?;
        if let Some(agent_identity) = agent_identity {
            stored = stored.with_agent_identity(agent_identity);
        }
        Ok(stored)
    }
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
    let codex_home = default_codex_home();
    let bindings = codex::profile_bindings(&codex_home, &state.profile_backup_root())?;
    let documents = current_codex_import_documents(&codex_home, &bindings)?;
    preview_account_import_documents(documents, &state).await
}

#[tauri::command]
pub async fn current_chatgpt_profile_available(
    state: State<'_, DesktopState>,
) -> CommandResult<bool> {
    let codex_home = default_codex_home();
    let bindings = codex::profile_bindings(&codex_home, &state.profile_backup_root())?;
    if current_codex_profile_is_managed(&bindings) {
        return Ok(false);
    }
    let auth_path = codex_home.join("auth.json");
    if !auth_path.is_file() {
        return Ok(false);
    }
    let documents = read_import_documents(vec![auth_path])?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(&state, &credentials)?;
    let Ok(parsed) = parse_import(
        &documents[0],
        Some("auth.json"),
        &existing.keys().cloned().collect::<Vec<_>>(),
    ) else {
        return Ok(false);
    };
    Ok(is_usable_current_chatgpt_profile(
        &parsed,
        current_time_ms(),
    ))
}

pub(super) async fn preview_account_import_documents(
    documents: Vec<String>,
    state: &DesktopState,
) -> CommandResult<ImportSessionResponse> {
    let _mutation = state.setup_guard().await;
    let (content, _) = normalize_import_input(StartAccountImportInput {
        content: None,
        documents,
        source_file: None,
    })?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .start(
            &content,
            None,
            &existing.keys().cloned().collect::<Vec<_>>(),
        )
        .map_err(import_session_error)?;
    let session_id = session.session_id.clone();
    let prepared = async {
        let (content, preview) =
            prepare_import_preview(state, &credentials, session, false).await?;
        sessions
            .prepare(
                &session_id,
                content.as_deref(),
                preview,
                &existing.keys().cloned().collect::<Vec<_>>(),
            )
            .map_err(import_session_error)
    }
    .await;
    match prepared {
        Ok(session) => Ok(session.into()),
        Err(error) => {
            let _ = sessions.cancel(&session_id);
            Err(error)
        }
    }
}

pub(super) fn current_codex_import_documents(
    codex_home: &Path,
    bindings: &[codex::ProfileBinding],
) -> LocalResult<Vec<String>> {
    if current_codex_profile_is_managed(bindings) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "the current ChatGPT profile is already managed by the local gateway",
        ));
    }
    let auth_path = codex_home.join("auth.json");
    if !auth_path.is_file() {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "the current ChatGPT profile was not found",
        ));
    }
    read_import_documents(vec![auth_path])
}

pub(super) fn current_codex_profile_is_managed(bindings: &[codex::ProfileBinding]) -> bool {
    bindings.iter().any(|binding| {
        binding.active && binding.credential_kind == codex::ProfileCredentialKind::LocalGateway
    })
}

pub(super) fn is_usable_current_chatgpt_profile(parsed: &ParsedImport, now_ms: u64) -> bool {
    let ([row], [item]) = (parsed.preview.rows.as_slice(), parsed.items.as_slice()) else {
        return false;
    };
    let identity = imported_identity(item.secrets().id_token(), item.secrets().access_token());
    let refreshable = item.secrets().refresh_token().is_some()
        || identity.access_expires_at_ms.is_some_and(|expires_at_ms| {
            expires_at_ms > now_ms.saturating_add(TOKEN_REFRESH_SKEW_MS)
        });
    row.auth_mode == ImportAuthMode::OAuth
        && row.status == ImportPreviewStatus::Ready
        && row.selectable
        && !row.existing
        && refreshable
        && (item.account_id.is_some() || identity.provider_account_id.is_some())
}

pub(crate) fn pick_account_import_documents(app: &AppHandle) -> CommandResult<Option<Vec<String>>> {
    let Some(files) = app
        .dialog()
        .file()
        .add_filter("Account files", &["json", "txt"])
        .blocking_pick_files()
    else {
        return Ok(None);
    };
    let paths = files
        .into_iter()
        .map(|file| {
            file.into_path().map_err(|_| {
                LocalPoolError::new(ErrorCode::InvalidState, "selected file path is invalid")
            })
        })
        .collect::<LocalResult<Vec<_>>>()?;
    read_import_documents(paths).map(Some).map_err(Into::into)
}

pub(crate) fn read_import_documents(paths: Vec<PathBuf>) -> LocalResult<Vec<String>> {
    if paths.is_empty() || paths.len() > MAX_IMPORT_ITEMS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("select between 1 and {MAX_IMPORT_ITEMS} import files"),
        ));
    }
    let mut total_bytes = 0usize;
    let mut documents = Vec::with_capacity(paths.len());
    for path in paths {
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                value.eq_ignore_ascii_case("json") || value.eq_ignore_ascii_case("txt")
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import file must use the .json or .txt extension",
            ));
        }
        let metadata = std::fs::metadata(&path).map_err(|_| {
            LocalPoolError::new(ErrorCode::Io, "failed to read selected import file")
        })?;
        if !metadata.is_file() {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import path is not a file",
            ));
        }
        let length = usize::try_from(metadata.len()).map_err(|_| {
            LocalPoolError::new(ErrorCode::InvalidState, "selected import file is too large")
        })?;
        total_bytes = total_bytes.checked_add(length).ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import files are too large",
            )
        })?;
        if total_bytes > MAX_IMPORT_BYTES {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import files are too large",
            ));
        }
        documents.push(std::fs::read_to_string(path).map_err(|_| {
            LocalPoolError::new(ErrorCode::Io, "failed to read selected import file")
        })?);
    }
    Ok(documents)
}

pub(super) fn normalize_import_input(
    input: StartAccountImportInput,
) -> CommandResult<(String, Option<String>)> {
    let content = input.content.filter(|value| !value.trim().is_empty());
    if !input.documents.is_empty() {
        if content.is_some() || input.source_file.is_some() {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "paste content and file documents cannot be imported together",
            )
            .into());
        }
        if input.documents.len() == 1 {
            return Ok((
                input
                    .documents
                    .into_iter()
                    .next()
                    .expect("one document exists"),
                None,
            ));
        }
        let content = combine_import_documents(&input.documents)
            .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.message))?;
        return Ok((content, None));
    }
    Ok((content.unwrap_or_default(), input.source_file))
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

pub(super) async fn confirm_local_account_import_inner(
    input: ConfirmAccountImportInput,
    state: &DesktopState,
    app: Option<&AppHandle>,
) -> CommandResult<ConfirmAccountImportResponse> {
    let selected_item_ids = normalize_selected_item_ids(input.selected_item_ids)?;
    let configured_models = normalize_models(input.models.clone())?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .resume(
            &input.session_id,
            &existing.keys().cloned().collect::<Vec<_>>(),
        )
        .map_err(import_session_error)?;
    let selected = selected_item_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let refresh_exchange_required = !session.prepared
        && session.items.iter().any(|item| {
            selected.contains(item.item_id.as_str())
                && item.secrets().access_token().is_none()
                && item.secrets().refresh_token().is_some()
        });
    let probe_quota = input.probe_quota
        && !session.preview.rows.iter().any(|row| {
            selected.contains(row.item_id.as_str())
                && row.auth_mode != ImportAuthMode::ApiKey
                && row.quota_status == ImportQuotaStatus::Skipped
        });
    if refresh_exchange_required {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "prepare refresh-only credentials before confirming selected accounts",
        )
        .into());
    }
    let row_context = session
        .preview
        .rows
        .iter()
        .map(|row| {
            (
                row.item_id.clone(),
                ImportRowContext {
                    label: row.label.clone(),
                    auth_mode: row.auth_mode,
                    selectable: row.selectable,
                    plan: row.plan.clone(),
                    subscription_active_until_ms: row
                        .subscription_expires_at
                        .as_deref()
                        .and_then(parse_subscription_timestamp_ms),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut items = session
        .items
        .into_iter()
        .map(|item| (item.item_id.clone(), item))
        .collect::<HashMap<_, _>>();
    let mut results = Vec::with_capacity(selected_item_ids.len());
    let total = selected_item_ids.len();
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    emit_account_import_progress(app, &input.session_id, 0, total, succeeded, failed, None);

    for (completed, item_id) in selected_item_ids.into_iter().enumerate() {
        let label = row_context
            .get(&item_id)
            .map(|context| context.label.clone())
            .unwrap_or_else(|| item_id.clone());
        emit_account_import_progress(
            app,
            &input.session_id,
            completed,
            total,
            succeeded,
            failed,
            Some(label),
        );
        let result = match row_context.get(&item_id) {
            None => ImportItemResult::failure(
                item_id,
                ImportItemError::new("item_not_found", "import item was not found"),
            ),
            Some(context) if !context.selectable => ImportItemResult::failure(
                item_id,
                ImportItemError::new("item_not_selectable", "import item cannot be selected"),
            ),
            Some(context) => match items.remove(&item_id) {
                None => ImportItemResult::failure(
                    item_id,
                    ImportItemError::new(
                        "item_not_selectable",
                        "import item has no usable credentials",
                    ),
                ),
                Some(item) if context.auth_mode == ImportAuthMode::ApiKey => {
                    match import_source_item(
                        state,
                        item,
                        input.add_to_pool,
                        input.discover_models,
                        &configured_models,
                    )
                    .await
                    {
                        Ok(source) => ImportItemResult::source_success(item_id, source),
                        Err(error) => ImportItemResult::failure(item_id, error),
                    }
                }
                Some(item) => match import_account_item(
                    state,
                    &credentials,
                    item,
                    context,
                    input.add_to_pool,
                    input.discover_models,
                    probe_quota,
                    &configured_models,
                )
                .await
                {
                    Ok((account, quota)) => {
                        ImportItemResult::account_success(item_id, account, quota)
                    }
                    Err(error) => ImportItemResult::failure(item_id, error),
                },
            },
        };
        match result.status {
            ImportItemStatus::Succeeded => succeeded += 1,
            ImportItemStatus::Failed => failed += 1,
        }
        results.push(result);
        emit_account_import_progress(
            app,
            &input.session_id,
            completed + 1,
            total,
            succeeded,
            failed,
            None,
        );
    }

    if failed == 0 {
        sessions
            .complete(&input.session_id)
            .map_err(import_session_error)?;
    }
    Ok(ConfirmAccountImportResponse {
        session_id: input.session_id,
        results,
    })
}

pub(super) fn emit_account_import_progress(
    app: Option<&AppHandle>,
    session_id: &str,
    completed: usize,
    total: usize,
    succeeded: usize,
    failed: usize,
    current_label: Option<String>,
) {
    if let Some(app) = app {
        let _ = app.emit(
            ACCOUNT_IMPORT_PROGRESS_EVENT,
            AccountImportProgressEvent {
                session_id: session_id.to_string(),
                completed,
                total,
                succeeded,
                failed,
                current_label,
            },
        );
    }
}

pub(super) async fn prepare_import_preview(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    session: ImportSession,
    probe_quota: bool,
) -> CommandResult<(Option<String>, ImportPreview)> {
    if session.items.len()
        != session
            .preview
            .rows
            .iter()
            .filter(|row| row.selectable)
            .count()
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "import preview does not match its credential items",
        )
        .into());
    }
    let mut preview = session.preview;
    let mut prepared_values = Vec::with_capacity(session.items.len());
    let mut credentials_changed = false;
    let now_ms = current_time_ms();
    let settings = state.store()?.gateway().clone();
    let common_proxy = common_proxy_config(&settings)?;
    for (item, row) in session
        .items
        .into_iter()
        .zip(preview.rows.iter_mut().filter(|row| row.selectable))
    {
        let original = parsed_item_value(&item, row.auth_mode, None);
        if row.auth_mode == ImportAuthMode::ApiKey {
            if let (Some(base_url), Some(api_key)) =
                (item.base_url.as_deref(), item.secrets().api_key())
            {
                if find_existing_source(state, base_url, api_key)
                    .map_err(import_item_command_error)?
                    .is_some()
                {
                    row.existing = true;
                    row.status = ImportPreviewStatus::Existing;
                }
            }
            prepared_values.push(original);
            continue;
        }

        let plan_hint = row.plan.clone();
        let hinted_proxy = hinted_import_proxy(state, credentials, &settings, &item)
            .map_err(import_item_command_error)?;
        let import_proxy = hinted_proxy.as_ref().or(common_proxy.as_ref());
        if let Err(error) = ensure_account_proxy(&settings, import_proxy) {
            row.status = ImportPreviewStatus::Invalid;
            row.selectable = false;
            row.default_selected = false;
            row.error = Some(ImportIssue {
                code: ImportIssueCode::RefreshExchangeFailed,
                message: error.message,
            });
            continue;
        }
        credentials_changed |=
            item.secrets().access_token().is_none() && item.secrets().refresh_token().is_some();
        let material = match build_import_credential_material(
            item,
            now_ms,
            plan_hint.as_deref(),
            row.subscription_expires_at
                .as_deref()
                .and_then(parse_subscription_timestamp_ms),
            import_proxy,
            settings.quota_request_timeout_seconds,
        )
        .await
        {
            Ok(material) => material,
            Err(error) => {
                row.status = ImportPreviewStatus::Invalid;
                row.selectable = false;
                row.default_selected = false;
                row.error = Some(ImportIssue {
                    code: ImportIssueCode::RefreshExchangeFailed,
                    message: error.message,
                });
                continue;
            }
        };
        let Some(provider_account_id) = material.provider_account_id.as_deref() else {
            row.status = ImportPreviewStatus::Invalid;
            row.selectable = false;
            row.default_selected = false;
            row.error = Some(ImportIssue {
                code: ImportIssueCode::InvalidCredentials,
                message: "ChatGPT account identity is missing".into(),
            });
            continue;
        };
        row.identity = masked_account_identity(provider_account_id);
        row.plan = material.plan_type.clone().or_else(|| row.plan.clone());
        row.expires_at = material.expires_at_ms.and_then(timestamp_from_ms);
        row.subscription_expires_at = material
            .subscription_active_until_ms
            .and_then(timestamp_from_ms)
            .or_else(|| row.subscription_expires_at.clone());
        let existing_account = find_existing_account(
            state,
            credentials,
            provider_account_id,
            material.provider_user_id.as_deref(),
            material.email.as_deref(),
        )
        .map_err(import_item_command_error)?;
        if existing_account.is_some() {
            row.existing = true;
            row.status = ImportPreviewStatus::Existing;
        }
        if probe_quota {
            let proxy = match existing_account {
                Some(ref account) => credentials
                    .load(&account.account.id)
                    .map_err(credential_local_error)?
                    .map(|stored| effective_proxy_config(&settings, &stored))
                    .transpose()?
                    .flatten()
                    .or_else(|| common_proxy.clone()),
                None => common_proxy.clone(),
            };
            let request_timeout = Duration::from_secs(settings.quota_request_timeout_seconds);
            let quota =
                CodexQuotaClient::new_with_proxy_and_timeout(proxy.as_ref(), request_timeout)
                    .map_err(|_| {
                        LocalPoolError::new(ErrorCode::InvalidState, "quota client is unavailable")
                    })?;
            match tokio::time::timeout(
                request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
                quota.refresh_data_with_subscription_authorization(
                    material
                        .authorization(now_ms)
                        .map_err(import_item_command_error)?,
                    material
                        .subscription_authorization()
                        .map_err(import_item_command_error)?,
                    provider_account_id,
                    now_ms,
                    &zenith_relay_core::quota::Subscription::normalize(
                        zenith_relay_core::quota::SubscriptionInput {
                            plan_type: material.plan_type.clone(),
                            active_until_ms: material.subscription_active_until_ms,
                            forbidden: false,
                            observed_at_ms: now_ms,
                        },
                    ),
                    true,
                ),
            )
            .await
            {
                Ok(Ok(data)) => match data.quota.normalize(&Default::default()) {
                    Ok((_, subscription)) => {
                        row.quota_status = ImportQuotaStatus::Success;
                        row.error = None;
                        if let Some(subscription) = subscription {
                            row.plan = subscription.plan_type.or_else(|| row.plan.clone());
                            row.subscription_expires_at = subscription
                                .active_until_ms
                                .and_then(timestamp_from_ms)
                                .or_else(|| row.subscription_expires_at.clone());
                        }
                    }
                    Err(_) => mark_preview_quota_failed(row, "quota response is invalid"),
                },
                Ok(Err(_)) => mark_preview_quota_failed(row, "quota probe failed"),
                Err(_) => mark_preview_quota_failed(row, "quota probe timed out"),
            }
        }
        prepared_values.push(parsed_item_value_from_material(original, &material));
    }
    let content = credentials_changed
        .then(|| serde_json::to_string(&prepared_values))
        .transpose()
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "failed to encode prepared import credentials",
            )
        })?;
    Ok((content, preview))
}

pub(super) fn mark_preview_quota_failed(
    row: &mut zenith_relay_core::accounts::ImportPreviewRow,
    message: &str,
) {
    row.quota_status = ImportQuotaStatus::Failed;
    row.status = ImportPreviewStatus::QuotaFailed;
    row.error = Some(ImportIssue {
        code: ImportIssueCode::QuotaProbeFailed,
        message: message.into(),
    });
}

pub(super) fn parsed_item_value(
    item: &ParsedImportItem,
    auth_mode: ImportAuthMode,
    material: Option<&ImportedCredentialMaterial>,
) -> serde_json::Value {
    let mut value = serde_json::Map::new();
    value.insert(
        "label".into(),
        serde_json::Value::String(item.label.clone()),
    );
    value.insert(
        "auth_mode".into(),
        serde_json::Value::String(
            match auth_mode {
                ImportAuthMode::OAuth => "oauth",
                ImportAuthMode::AgentIdentity => "agent_identity",
                ImportAuthMode::ApiKey => "api_key",
                ImportAuthMode::ImportedToken => "imported_token",
                ImportAuthMode::Unknown => "unknown",
            }
            .into(),
        ),
    );
    insert_optional_string(&mut value, "account_id", item.account_id.as_deref());
    insert_optional_string(&mut value, "user_id", item.chatgpt_user_id.as_deref());
    insert_optional_string(
        &mut value,
        "organization_id",
        item.organization_id.as_deref(),
    );
    insert_optional_string(&mut value, "base_url", item.base_url.as_deref());
    insert_optional_string(&mut value, "protocol", item.protocol.as_deref());
    insert_optional_string(&mut value, "email", item.email());
    if let Some(priority) = item.priority {
        value.insert("priority".into(), priority.into());
    }
    let secrets = item.secrets();
    insert_optional_string(&mut value, "access_token", secrets.access_token());
    insert_optional_string(&mut value, "refresh_token", secrets.refresh_token());
    insert_optional_string(&mut value, "id_token", secrets.id_token());
    insert_optional_string(&mut value, "api_key", secrets.api_key());
    insert_optional_string(&mut value, "agent_private_key", secrets.agent_private_key());
    insert_optional_string(&mut value, "agent_runtime_id", secrets.agent_runtime_id());
    insert_optional_string(&mut value, "task_id", secrets.agent_task_id());
    if let Some(material) = material {
        insert_optional_string(
            &mut value,
            "account_id",
            material.provider_account_id.as_deref(),
        );
        insert_optional_string(&mut value, "user_id", material.provider_user_id.as_deref());
        insert_optional_string(
            &mut value,
            "organization_id",
            material.organization_id.as_deref(),
        );
        insert_optional_string(&mut value, "email", material.email.as_deref());
        insert_optional_string(&mut value, "access_token", Some(&material.access_token));
        if let Some(agent) = material.agent_identity.as_ref() {
            insert_optional_string(&mut value, "agent_private_key", Some(agent.private_key()));
            insert_optional_string(&mut value, "agent_runtime_id", Some(agent.runtime_id()));
            insert_optional_string(&mut value, "task_id", agent.task_id());
        }
        insert_optional_string(
            &mut value,
            "refresh_token",
            material.refresh_token.as_deref(),
        );
        insert_optional_string(&mut value, "id_token", material.id_token.as_deref());
        insert_optional_string(&mut value, "plan_type", material.plan_type.as_deref());
        if let Some(expires_at_ms) = material.expires_at_ms {
            value.insert("expires_at_ms".into(), expires_at_ms.into());
        }
    }
    serde_json::Value::Object(value)
}

pub(super) fn parsed_item_value_from_material(
    original: serde_json::Value,
    material: &ImportedCredentialMaterial,
) -> serde_json::Value {
    let mut value = original.as_object().cloned().unwrap_or_default();
    insert_optional_string(
        &mut value,
        "account_id",
        material.provider_account_id.as_deref(),
    );
    insert_optional_string(&mut value, "user_id", material.provider_user_id.as_deref());
    insert_optional_string(
        &mut value,
        "organization_id",
        material.organization_id.as_deref(),
    );
    insert_optional_string(&mut value, "email", material.email.as_deref());
    insert_optional_string(&mut value, "access_token", Some(&material.access_token));
    if let Some(agent) = material.agent_identity.as_ref() {
        insert_optional_string(&mut value, "agent_private_key", Some(agent.private_key()));
        insert_optional_string(&mut value, "agent_runtime_id", Some(agent.runtime_id()));
        insert_optional_string(&mut value, "task_id", agent.task_id());
    }
    insert_optional_string(
        &mut value,
        "refresh_token",
        material.refresh_token.as_deref(),
    );
    insert_optional_string(&mut value, "id_token", material.id_token.as_deref());
    insert_optional_string(&mut value, "plan_type", material.plan_type.as_deref());
    if let Some(expires_at_ms) = material.expires_at_ms {
        value.insert("expires_at_ms".into(), expires_at_ms.into());
    }
    serde_json::Value::Object(value)
}

pub(super) fn insert_optional_string(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        object.insert(key.into(), serde_json::Value::String(value.to_string()));
    }
}

pub(crate) async fn stage_returned_remote_account(
    state: &DesktopState,
    local_account_id: &str,
    content: &str,
) -> LocalResult<LocalAccountRecord> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let mut parsed = parse_import(content, None, &existing.keys().cloned().collect::<Vec<_>>())
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    if parsed.items.len() != 1 || parsed.preview.rows.len() != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote account export must contain exactly one account",
        ));
    }
    let row = parsed.preview.rows.remove(0);
    if !row.selectable {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote account export is not usable",
        ));
    }
    let existing_record = state
        .store()?
        .account(local_account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local account not found"))?;
    let context = ImportRowContext {
        label: row.label,
        auth_mode: row.auth_mode,
        selectable: row.selectable,
        plan: row.plan,
        subscription_active_until_ms: row
            .subscription_expires_at
            .as_deref()
            .and_then(parse_subscription_timestamp_ms),
    };
    let configured_models = existing_record.models.clone();
    let item = parsed.items.remove(0);
    let (account, _) = import_account_item(
        state,
        &credentials,
        item,
        &context,
        false,
        true,
        true,
        &configured_models,
    )
    .await
    .map_err(|error| {
        LocalPoolError::new(
            if error.code == "recovery_required" {
                ErrorCode::RecoveryRequired
            } else {
                ErrorCode::InvalidState
            },
            error.message,
        )
    })?;
    if account.account.id != local_account_id
        || account.remote_location != existing_record.remote_location
        || account.account.enabled
        || account.account.in_pool
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "returned credentials did not stage on the expected inactive local account",
        ));
    }
    Ok(account)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn import_account_item(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    item: ParsedImportItem,
    context: &ImportRowContext,
    add_to_pool: bool,
    discover_models: bool,
    probe_quota: bool,
    configured_models: &[String],
) -> ItemResult<(LocalAccountRecord, AccountQuotaOutcome)> {
    ensure_account_import_item(&item)?;
    let issued_at_ms = current_time_ms();
    let item_label = item.label.clone();
    let item_priority = item.priority;
    let settings = state
        .store()
        .map_err(|_| ImportItemError::new("account_store_failed", "account store is unavailable"))?
        .gateway()
        .clone();
    let common_proxy = common_proxy_config(&settings).map_err(proxy_item_error)?;
    let hinted_proxy = hinted_import_proxy(state, credential_store, &settings, &item)?;
    let import_proxy = hinted_proxy.as_ref().or(common_proxy.as_ref());
    ensure_account_proxy(&settings, import_proxy).map_err(proxy_item_error)?;
    let mut material = build_import_credential_material(
        item,
        issued_at_ms,
        context.plan.as_deref(),
        context.subscription_active_until_ms,
        import_proxy,
        settings.quota_request_timeout_seconds,
    )
    .await?;
    let provider_account_id = material.provider_account_id.as_deref().ok_or_else(|| {
        ImportItemError::new(
            "provider_account_id_missing",
            "ChatGPT account id is missing from imported credentials",
        )
    })?;
    let existing_account = find_existing_account(
        state,
        credential_store,
        provider_account_id,
        material.provider_user_id.as_deref(),
        material.email.as_deref(),
    )?;
    let local_account_id = existing_account
        .as_ref()
        .map(|account| account.account.id.clone())
        .unwrap_or_else(|| format!("account_{}", Uuid::new_v4().simple()));
    let old_credential = credential_store
        .load(&local_account_id)
        .map_err(credential_item_error)?;
    let preserved_refresh_token = material.refresh_token.is_none()
        && old_credential
            .as_ref()
            .and_then(StoredCodexCredentials::refresh_token)
            .is_some();
    if preserved_refresh_token {
        material.refresh_token = old_credential
            .as_ref()
            .and_then(StoredCodexCredentials::refresh_token)
            .map(str::to_string);
    }
    let generation = old_credential
        .as_ref()
        .map(StoredCodexCredentials::generation)
        .into_iter()
        .chain(
            existing_account
                .as_ref()
                .map(|account| account.account.token_generation),
        )
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let subscription_active_until_ms = material.subscription_active_until_ms;
    let mut credentials = material.into_stored(&local_account_id, issued_at_ms, generation)?;
    if let Some(proxy_url) = old_credential
        .as_ref()
        .and_then(StoredCodexCredentials::proxy_url)
    {
        credentials = credentials
            .with_proxy_url(Some(proxy_url.to_string()))
            .map_err(credential_item_error)?;
    }
    let proxy = effective_proxy_config(&settings, &credentials).map_err(proxy_item_error)?;
    let provider_account_id = credentials.provider_account_id().ok_or_else(|| {
        ImportItemError::new(
            "provider_account_id_missing",
            "ChatGPT account id is missing from imported credentials",
        )
    })?;
    let identity_is_registered = credentials
        .agent_identity()
        .is_none_or(|agent| agent.task_id().is_some());
    let models = if discover_models && identity_is_registered {
        let client = CodexModelsClient::new_with_proxy(proxy.as_ref()).map_err(model_item_error)?;
        let models = client
            .discover_authorized(
                credentials
                    .authorization(issued_at_ms)
                    .map_err(credential_item_error)?,
                provider_account_id,
                zenith_relay_core::providers::chatgpt::CODEX_MODELS_CLIENT_VERSION,
            )
            .await
            .map_err(model_item_error)?;
        if models.is_empty() {
            return Err(ImportItemError::new(
                "models_empty",
                "ChatGPT account did not expose any supported models",
            ));
        }
        models
    } else if !configured_models.is_empty() {
        configured_models.to_vec()
    } else if let Some(existing) = &existing_account {
        existing.models.clone()
    } else {
        Vec::new()
    };
    let auth_mode = if preserved_refresh_token {
        existing_account
            .as_ref()
            .map(|account| account.account.auth_mode)
            .unwrap_or(account_auth_mode(context.auth_mode)?)
    } else {
        account_auth_mode(context.auth_mode)?
    };
    let priority = existing_account
        .as_ref()
        .map(|value| value.priority)
        .or(item_priority);
    let mut account = records::new_account_record(
        &credentials,
        auth_mode,
        models,
        priority.unwrap_or_default(),
        issued_at_ms,
    )
    .map_err(|_| ImportItemError::new("invalid_account", "imported account record is invalid"))?;
    merge_existing_account(&mut account, existing_account.as_ref());
    account.account.in_pool |= add_to_pool;
    if let Some(active_until_ms) = subscription_active_until_ms {
        account.account.subscription = zenith_relay_core::quota::Subscription::normalize(
            zenith_relay_core::quota::SubscriptionInput {
                plan_type: account.account.subscription.plan_type.clone(),
                active_until_ms: Some(active_until_ms),
                forbidden: false,
                observed_at_ms: issued_at_ms,
            },
        );
    }
    if existing_account.is_none() && !item_label.trim().is_empty() {
        account.account.label = item_label;
    }
    validate_label(&account.account.label)
        .map_err(|_| ImportItemError::new("invalid_label", "imported account label is invalid"))?;
    account.normalize();
    let quota = if probe_quota && identity_is_registered {
        probe_import_quota(
            &mut account,
            &credentials,
            proxy.as_ref(),
            settings.quota_request_timeout_seconds,
        )
        .await
    } else {
        AccountQuotaOutcome::Skipped
    };
    persist_imported_account(
        state,
        credential_store,
        &credentials,
        old_credential.as_ref(),
        account.clone(),
    )
    .await?;
    Ok((account, quota))
}

pub(super) async fn build_import_credential_material(
    item: ParsedImportItem,
    issued_at_ms: u64,
    plan_hint: Option<&str>,
    subscription_active_until_hint: Option<u64>,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> ItemResult<ImportedCredentialMaterial> {
    let email = item.email().map(str::to_string);
    let item_account_id = item.account_id.clone();
    let item_user_id = item.chatgpt_user_id.clone();
    let organization_id = item.organization_id.clone();
    let secrets = item.into_secrets();
    let original_refresh = secrets.refresh_token().map(str::to_string);
    let imported_identity = imported_identity(secrets.id_token(), secrets.access_token());

    let agent_identity = match (secrets.agent_private_key(), secrets.agent_runtime_id()) {
        (Some(private_key), Some(runtime_id)) => Some(
            match secrets.agent_task_id() {
                Some(task_id) => AgentIdentityCredential::new(
                    private_key.to_string(),
                    runtime_id.to_string(),
                    task_id.to_string(),
                ),
                None => AgentIdentityCredential::unregistered(
                    private_key.to_string(),
                    runtime_id.to_string(),
                ),
            }
            .map_err(|_| {
                ImportItemError::new(
                    "agent_identity_invalid",
                    "Agent Identity credential is invalid",
                )
            })?,
        ),
        (None, None) => None,
        _ => {
            return Err(ImportItemError::new(
                "agent_identity_invalid",
                "Agent Identity credential is incomplete",
            ))
        }
    };

    if let Some(access_token) = secrets.access_token() {
        let material = ImportedCredentialMaterial {
            access_token: access_token.to_string(),
            agent_identity,
            refresh_token: original_refresh,
            id_token: secrets.id_token().map(str::to_string),
            expires_at_ms: imported_identity.access_expires_at_ms,
            email: email.or(imported_identity.email),
            provider_account_id: imported_identity.provider_account_id.or(item_account_id),
            provider_user_id: imported_identity.provider_user_id.or(item_user_id),
            organization_id,
            plan_type: imported_identity
                .plan_type
                .or_else(|| plan_hint.map(str::to_string)),
            subscription_active_until_ms: imported_identity
                .subscription_active_until_ms
                .or(subscription_active_until_hint),
            account_is_fedramp: imported_identity.account_is_fedramp,
        };
        return resolve_import_account_identity(material, proxy, request_timeout_seconds).await;
    }

    let Some(refresh_token) = original_refresh else {
        let agent_identity = agent_identity.ok_or_else(|| {
            ImportItemError::new(
                "access_token_missing",
                "ChatGPT account import requires an access or refresh token",
            )
        })?;
        return Ok(ImportedCredentialMaterial {
            access_token: String::new(),
            agent_identity: Some(agent_identity),
            refresh_token: None,
            id_token: None,
            expires_at_ms: None,
            email: email.or(imported_identity.email),
            provider_account_id: imported_identity.provider_account_id.or(item_account_id),
            provider_user_id: imported_identity.provider_user_id.or(item_user_id),
            organization_id,
            plan_type: imported_identity
                .plan_type
                .or_else(|| plan_hint.map(str::to_string)),
            subscription_active_until_ms: imported_identity
                .subscription_active_until_ms
                .or(subscription_active_until_hint),
            account_is_fedramp: imported_identity.account_is_fedramp,
        });
    };
    let oauth = CodexOAuthClient::new_with_proxy(proxy).map_err(|_| {
        ImportItemError::new(
            "refresh_exchange_unavailable",
            "refresh-token exchange is unavailable",
        )
    })?;
    let tokens = oauth
        .exchange_refresh_token(&refresh_token, issued_at_ms)
        .await
        .map_err(|failure| ImportItemError::new(&failure.code, "refresh-token exchange failed"))?;
    let oauth_claims = tokens.identity_claims().map_err(|_| {
        ImportItemError::new(
            "invalid_identity_token",
            "refreshed identity token is invalid",
        )
    })?;
    let oauth_email = oauth_claims
        .as_ref()
        .and_then(|claims| claims.email().map(str::to_string));
    let oauth_account_id = oauth_claims
        .as_ref()
        .and_then(|claims| claims.account_id().map(str::to_string));
    let oauth_user_id = oauth_claims
        .as_ref()
        .and_then(|claims| claims.user_id().map(str::to_string));
    let oauth_plan = oauth_claims
        .as_ref()
        .and_then(|claims| claims.plan_type().map(str::to_string));
    let oauth_subscription_active_until_ms = oauth_claims
        .as_ref()
        .and_then(|claims| claims.subscription_active_until_ms());
    let account_is_fedramp = oauth_claims
        .as_ref()
        .is_some_and(|claims| claims.account_is_fedramp());
    let (access_token, rotated_refresh, id_token, expires_at_ms) = tokens.into_secret_parts();
    let material = ImportedCredentialMaterial {
        access_token,
        agent_identity,
        refresh_token: rotated_refresh.or(Some(refresh_token)),
        id_token,
        expires_at_ms,
        email: email.or(oauth_email).or(imported_identity.email),
        provider_account_id: oauth_account_id
            .or(imported_identity.provider_account_id)
            .or(item_account_id),
        provider_user_id: oauth_user_id
            .or(imported_identity.provider_user_id)
            .or(item_user_id),
        organization_id,
        plan_type: oauth_plan
            .or(imported_identity.plan_type)
            .or_else(|| plan_hint.map(str::to_string)),
        subscription_active_until_ms: imported_identity
            .subscription_active_until_ms
            .or(oauth_subscription_active_until_ms)
            .or(subscription_active_until_hint),
        account_is_fedramp: account_is_fedramp || imported_identity.account_is_fedramp,
    };
    resolve_import_account_identity(material, proxy, request_timeout_seconds).await
}

pub(super) async fn resolve_import_account_identity(
    mut material: ImportedCredentialMaterial,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> ItemResult<ImportedCredentialMaterial> {
    if material.provider_account_id.is_some() {
        return Ok(material);
    }
    let endpoint = Url::parse(CODEX_ACCOUNT_CHECK_ENDPOINT).map_err(|_| {
        ImportItemError::new(
            "provider_account_lookup_failed",
            "ChatGPT account lookup is unavailable",
        )
    })?;
    material.provider_account_id = Some(
        lookup_import_account_id(
            endpoint,
            &material.access_token,
            proxy,
            Duration::from_secs(request_timeout_seconds.max(1)),
        )
        .await?,
    );
    Ok(material)
}

pub(super) async fn lookup_import_account_id(
    endpoint: Url,
    access_token: &str,
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
) -> ItemResult<String> {
    let authorization = HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| {
        ImportItemError::new("access_token_rejected", "imported access token is invalid")
    })?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(timeout)
        .user_agent("Zenith Relay");
    let http = match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| {
        ImportItemError::new(
            "provider_account_lookup_failed",
            "ChatGPT account lookup client could not be created",
        )
    })?;
    let response = http
        .get(endpoint)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| {
            ImportItemError::new(
                "provider_account_lookup_failed",
                "ChatGPT account lookup request failed",
            )
        })?;
    let status = response.status();
    let body = collect_limited(response, MAX_ACCOUNT_PROFILE_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            LimitedBodyError::Transport => ImportItemError::new(
                "provider_account_lookup_failed",
                "ChatGPT account lookup response could not be read",
            ),
            LimitedBodyError::TooLarge => ImportItemError::new(
                "provider_account_lookup_failed",
                "ChatGPT account lookup response was too large",
            ),
        })?;
    if !status.is_success() {
        let (code, message) = match status.as_u16() {
            401 | 403 => (
                "access_token_rejected",
                "ChatGPT rejected the imported access token",
            ),
            429 => (
                "account_profile_rate_limited",
                "ChatGPT rate limited the account lookup request",
            ),
            _ => (
                "provider_account_lookup_failed",
                "ChatGPT account lookup returned an unexpected status",
            ),
        };
        return Err(ImportItemError::new(code, message));
    }
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        ImportItemError::new(
            "provider_account_lookup_failed",
            "ChatGPT account lookup returned invalid JSON",
        )
    })?;
    account_id_from_check_response(&payload).ok_or_else(|| {
        ImportItemError::new(
            "provider_account_id_missing",
            "ChatGPT account lookup did not return an account id",
        )
    })
}

pub(super) fn hinted_import_proxy(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    settings: &crate::local_pool::models::GatewaySettings,
    item: &ParsedImportItem,
) -> ItemResult<Option<ProxyConfig>> {
    let Some(provider_account_id) = item.account_id.as_deref() else {
        return Ok(None);
    };
    let Some(existing) = find_existing_account(
        state,
        credential_store,
        provider_account_id,
        item.chatgpt_user_id.as_deref(),
        item.email(),
    )?
    else {
        return Ok(None);
    };
    let Some(credentials) = credential_store
        .load(&existing.account.id)
        .map_err(credential_item_error)?
    else {
        return Ok(None);
    };
    effective_proxy_config(settings, &credentials).map_err(proxy_item_error)
}

pub(super) async fn probe_import_quota(
    account: &mut LocalAccountRecord,
    credentials: &StoredCodexCredentials,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> AccountQuotaOutcome {
    let now_ms = current_time_ms();
    let Some(provider_account_id) = credentials.provider_account_id() else {
        let failure = QuotaRefreshFailure::new("invalid_chatgpt_account_id", false);
        apply_quota_failure(account, &failure, now_ms);
        return AccountQuotaOutcome::Failed {
            code: failure.code,
            retryable: failure.retryable,
        };
    };
    let request_timeout = Duration::from_secs(request_timeout_seconds);
    let client = match CodexQuotaClient::new_with_proxy_and_timeout(proxy, request_timeout) {
        Ok(client) => client,
        Err(failure) => {
            apply_quota_failure(account, &failure, now_ms);
            return AccountQuotaOutcome::Failed {
                code: failure.code,
                retryable: failure.retryable,
            };
        }
    };
    let outcome = match tokio::time::timeout(
        request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
        client.refresh_quota_authorized(
            match credentials.authorization(now_ms) {
                Ok(authorization) => authorization,
                Err(_) => {
                    let failure = QuotaRefreshFailure::new("invalid_access_token", false);
                    apply_quota_failure(account, &failure, now_ms);
                    return AccountQuotaOutcome::Failed {
                        code: failure.code,
                        retryable: failure.retryable,
                    };
                }
            },
            provider_account_id,
            now_ms,
            &account.account.subscription,
            true,
        ),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => QuotaRefreshOutcome::Failed {
            failure: QuotaRefreshFailure::new("quota_timeout", true),
            subscription: account.account.subscription.clone(),
        },
    };
    apply_quota_outcome(account, outcome, now_ms)
}

pub(super) fn apply_quota_outcome(
    account: &mut LocalAccountRecord,
    outcome: QuotaRefreshOutcome,
    now_ms: u64,
) -> AccountQuotaOutcome {
    match outcome {
        QuotaRefreshOutcome::Updated(data) => match apply_quota_success(account, data) {
            Ok(applied) => AccountQuotaOutcome::Updated {
                transitions: applied.transitions,
            },
            Err(_) => {
                let failure = QuotaRefreshFailure::new("quota_invalid_response", false);
                apply_quota_failure(account, &failure, now_ms);
                AccountQuotaOutcome::Failed {
                    code: failure.code,
                    retryable: failure.retryable,
                }
            }
        },
        QuotaRefreshOutcome::Failed {
            failure,
            subscription,
        } => {
            account.account.subscription = subscription;
            apply_quota_failure(account, &failure, now_ms);
            AccountQuotaOutcome::Failed {
                code: failure.code,
                retryable: failure.retryable,
            }
        }
    }
}

pub(super) fn apply_model_discovery(
    account: &mut LocalAccountRecord,
    result: std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) {
    match result {
        Ok(models) if !models.is_empty() => {
            account.models = models;
            account.normalize();
            let recovered = account
                .account
                .last_error_code
                .as_deref()
                .is_some_and(|code| code.starts_with("models_"));
            if recovered {
                account.account.last_error_code = None;
                if !matches!(
                    account.account.auth_state,
                    AccountAuthState::RequiresReauth(_)
                ) {
                    account.account.health = AccountHealthState::Healthy;
                }
            }
        }
        Ok(_) if account.models.is_empty() => {
            apply_model_discovery_failure(account, "models_empty", false)
        }
        Err(error)
            if account.models.is_empty()
                || matches!(
                    error.code,
                    ModelDiscoveryFailureCode::Unauthorized
                        | ModelDiscoveryFailureCode::InvalidAccessToken
                        | ModelDiscoveryFailureCode::InvalidAccountId
                ) =>
        {
            apply_model_discovery_failure(account, model_failure_code(&error), error.retryable)
        }
        Ok(_) | Err(_) => {}
    }
}

pub(super) fn apply_model_discovery_failure(
    account: &mut LocalAccountRecord,
    code: &str,
    retryable: bool,
) {
    account.account.last_error_code = Some(code.to_string());
    match code {
        "models_unauthorized" | "models_invalid_access_token" | "models_invalid_account_id" => {
            account.account.auth_state = AccountAuthState::Error;
            account.account.health = AccountHealthState::Unhealthy;
        }
        "models_forbidden" => account.account.health = AccountHealthState::Blocked,
        _ if retryable => account.account.health = AccountHealthState::Degraded,
        _ => account.account.health = AccountHealthState::Unhealthy,
    }
}

pub(super) async fn persist_imported_account(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    credentials: &StoredCodexCredentials,
    old_credential: Option<&StoredCodexCredentials>,
    account: LocalAccountRecord,
) -> ItemResult<()> {
    let (old_accounts, old_keys) = current_account_records(state).map_err(|_| {
        ImportItemError::new("account_store_failed", "account store is unavailable")
    })?;
    let sync_gateway = !account.models.is_empty();
    credential_store
        .save(credentials)
        .map_err(credential_item_error)?;
    if state
        .store()
        .and_then(|mut store| store.upsert_account(account.clone()))
        .is_err()
    {
        restore_credential_item(
            credential_store,
            credentials.local_account_id(),
            old_credential,
        )?;
        return Err(ImportItemError::new(
            "account_store_failed",
            "failed to save account record",
        ));
    }
    if sync_gateway
        && sync_accounts_or_rollback(state, old_accounts.clone(), old_keys.clone())
            .await
            .is_err()
    {
        restore_credential_item(
            credential_store,
            credentials.local_account_id(),
            old_credential,
        )?;
        repair_gateway_after_item_restore(state, old_accounts, old_keys).await?;
        return Err(ImportItemError::new(
            "gateway_sync_failed",
            "failed to apply account to the local gateway",
        ));
    }
    if credentials.has_oauth()
        && state
            .token_authority()
            .register(
                credentials.local_account_id(),
                credentials.to_token_set().map_err(credential_item_error)?,
                account.account.auth_state,
            )
            .await
            .is_err()
    {
        rollback_after_authority_failure(
            state,
            credential_store,
            credentials.local_account_id(),
            old_credential,
            old_accounts,
            old_keys,
        )
        .await?;
        return Err(ImportItemError::new(
            "token_authority_failed",
            "failed to register account token state",
        ));
    }
    if state
        .sync_account_quota_refresh(credentials.local_account_id(), current_time_ms())
        .is_err()
    {
        rollback_after_authority_failure(
            state,
            credential_store,
            credentials.local_account_id(),
            old_credential,
            old_accounts,
            old_keys,
        )
        .await?;
        return Err(ImportItemError::new(
            "quota_queue_failed",
            "failed to schedule account quota refresh",
        ));
    }
    Ok(())
}

pub(super) async fn rollback_after_authority_failure(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> ItemResult<()> {
    let (new_accounts, new_keys) = current_account_records(state)
        .map_err(|_| ImportItemError::recovery("failed to read account state during rollback"))?;
    restore_credential_item(credential_store, account_id, old_credential)?;
    state
        .store()
        .and_then(|mut store| {
            store.replace_accounts_and_keys(old_accounts.clone(), old_keys.clone())
        })
        .map_err(|_| {
            ImportItemError::recovery("failed to restore account records after registration error")
        })?;
    sync_accounts_or_rollback(state, new_accounts, new_keys)
        .await
        .map_err(|_| {
            ImportItemError::recovery("failed to restore gateway after registration error")
        })?;
    restore_authority(state, account_id, old_credential, &old_accounts).await?;
    Ok(())
}

pub(super) async fn restore_authority(
    state: &DesktopState,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    old_accounts: &[LocalAccountRecord],
) -> ItemResult<()> {
    let Some(credentials) = old_credential else {
        state.token_authority().remove(account_id);
        return Ok(());
    };
    if !credentials.has_oauth() {
        state.token_authority().remove(account_id);
        return Ok(());
    }
    let auth_state = old_accounts
        .iter()
        .find(|account| account.account.id == account_id)
        .map_or(AccountAuthState::Unknown, |account| {
            account.account.auth_state
        });
    state
        .token_authority()
        .register(
            account_id,
            credentials.to_token_set().map_err(credential_item_error)?,
            auth_state,
        )
        .await
        .map_err(|_| ImportItemError::recovery("failed to restore previous token authority state"))
}

pub(super) fn apply_account_patch(
    account: &mut LocalAccountRecord,
    input: UpdateAccountInput,
) -> LocalResult<()> {
    if let Some(label) = input.label {
        validate_label(&label)?;
        account.account.label = label;
    }
    if let Some(priority) = input.priority {
        account.priority = priority;
    }
    if let Some(weight) = input.weight {
        if weight == 0 {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "account weight must be positive",
            ));
        }
        account.weight = weight;
    }
    if let Some(models) = input.allowed_models {
        account.allowed_models = normalize_models(models)?;
    }
    if let Some(models) = input.excluded_models {
        account.excluded_models = normalize_models(models)?;
    }
    if let Some(in_pool) = input.in_pool {
        account.account.in_pool = in_pool;
    }
    if let Some(draining) = input.draining {
        account.account.draining = draining;
    }
    if let Some(purchase_cost) = input.purchase_cost_micro_usd {
        if purchase_cost > MAX_PURCHASE_COST_MICRO_USD {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "account purchase cost is too large",
            ));
        }
        account.purchase_cost_micro_usd = (purchase_cost > 0).then_some(purchase_cost);
    }
    account.normalize();
    Ok(())
}

pub(super) fn validate_account_record(account: &LocalAccountRecord) -> LocalResult<()> {
    validate_label(&account.account.label)?;
    if let Some(location) = &account.remote_location {
        if location.server_id.is_empty() || location.remote_account_id.is_empty() {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "remote account location is invalid",
            ));
        }
        if account.account.enabled || account.account.in_pool {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "an account managed by a remote server cannot run locally",
            ));
        }
    }
    if !account_model_state_is_valid(account) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "a healthy account must expose at least one model",
        ));
    }
    normalize_models(account.models.clone())?;
    normalize_models(account.allowed_models.clone())?;
    normalize_models(account.excluded_models.clone())?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend)
        .require(&account.account.id)
        .map_err(credential_local_error)?;
    if credentials.provider_account_id().is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account credentials do not contain a provider account id",
        ));
    }
    if credentials.has_oauth() {
        credentials.to_token_set().map_err(credential_local_error)?;
    }
    Ok(())
}

pub(super) fn existing_identity_index(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
) -> LocalResult<HashMap<String, String>> {
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let mut existing = HashMap::new();
    for account_id in account_ids {
        let Some(credentials) = credential_store
            .load(&account_id)
            .map_err(credential_local_error)?
        else {
            continue;
        };
        let Some(provider_account_id) = credentials.provider_account_id() else {
            continue;
        };
        existing
            .entry(provider_identity_key(
                provider_account_id,
                credentials.provider_user_id(),
                credentials.email(),
            ))
            .or_insert(account_id);
    }
    Ok(existing)
}

pub(super) fn find_existing_account(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    provider_account_id: &str,
    provider_user_id: Option<&str>,
    email: Option<&str>,
) -> ItemResult<Option<LocalAccountRecord>> {
    let accounts = state
        .store()
        .map_err(|_| ImportItemError::new("account_store_failed", "account store is unavailable"))?
        .accounts()
        .to_vec();
    let target = records::identity_hash(provider_account_id, provider_user_id, email);
    let direct = accounts
        .iter()
        .filter(|account| {
            account.account.source_id == records::CODEX_SOURCE_ID
                && account.account.identity.identity_hash == target
        })
        .cloned()
        .collect::<Vec<_>>();
    if direct.len() > 1 {
        return Err(ImportItemError::recovery(
            "multiple local accounts have the same ChatGPT identity",
        ));
    }
    if let Some(account) = direct.into_iter().next() {
        return Ok(Some(account));
    }
    let mut matching = Vec::new();
    for account in accounts {
        if account.account.source_id != records::CODEX_SOURCE_ID {
            continue;
        }
        let Some(credentials) = credential_store
            .load(&account.account.id)
            .map_err(credential_item_error)?
        else {
            continue;
        };
        let Some(account_id) = credentials.provider_account_id() else {
            continue;
        };
        if records::identity_hash(
            account_id,
            credentials.provider_user_id(),
            credentials.email(),
        ) == target
        {
            matching.push(account);
        }
    }
    if matching.len() > 1 {
        return Err(ImportItemError::recovery(
            "multiple local accounts have the same ChatGPT identity",
        ));
    }
    Ok(matching.pop())
}

pub(super) fn credential_item_error(error: CredentialError) -> ImportItemError {
    let code = match error.code {
        CredentialErrorCode::InvalidIdentity => "invalid_account_identity",
        CredentialErrorCode::InvalidSecret | CredentialErrorCode::InvalidVersion => {
            "invalid_credentials"
        }
        CredentialErrorCode::SecretMissing => "credentials_missing",
        CredentialErrorCode::SecretStoreUnavailable => "credential_store_unavailable",
    };
    ImportItemError::new(code, &error.message)
}

pub(super) fn import_item_command_error(error: ImportItemError) -> CommandError {
    let code = if error.code == "recovery_required" {
        ErrorCode::RecoveryRequired
    } else {
        ErrorCode::InvalidState
    };
    LocalPoolError::new(code, error.message).into()
}

pub(super) fn credential_local_error(error: CredentialError) -> LocalPoolError {
    let code = match error.code {
        CredentialErrorCode::SecretMissing => ErrorCode::NotFound,
        CredentialErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        _ => ErrorCode::InvalidState,
    };
    LocalPoolError::new(code, error.message)
}

pub(super) fn proxy_item_error(error: LocalPoolError) -> ImportItemError {
    ImportItemError::new("proxy_unavailable", &error.message)
}

pub(super) fn model_item_error(error: ModelDiscoveryFailure) -> ImportItemError {
    ImportItemError::new(model_failure_code(&error), &error.to_string())
}

pub(super) fn model_failure_code(error: &ModelDiscoveryFailure) -> &'static str {
    match error.code {
        ModelDiscoveryFailureCode::AgentTaskInvalid => "models_agent_task_invalid",
        ModelDiscoveryFailureCode::Forbidden => "models_forbidden",
        ModelDiscoveryFailureCode::HttpStatus => "models_http_status",
        ModelDiscoveryFailureCode::InvalidAccessToken => "models_invalid_access_token",
        ModelDiscoveryFailureCode::InvalidAccountId => "models_invalid_account_id",
        ModelDiscoveryFailureCode::InvalidClientVersion => "models_invalid_client_version",
        ModelDiscoveryFailureCode::InvalidEndpoint => "models_invalid_endpoint",
        ModelDiscoveryFailureCode::InvalidResponse => "models_invalid_response",
        ModelDiscoveryFailureCode::RateLimited => "models_rate_limited",
        ModelDiscoveryFailureCode::ResponseTooLarge => "models_response_too_large",
        ModelDiscoveryFailureCode::Transport => "models_transport",
        ModelDiscoveryFailureCode::Unauthorized => "models_unauthorized",
        ModelDiscoveryFailureCode::Upstream => "models_upstream",
    }
}

pub(super) fn import_session_error(error: ImportSessionError) -> CommandError {
    let code = match error.code {
        ImportSessionErrorCode::SessionNotFound => ErrorCode::NotFound,
        ImportSessionErrorCode::SecretMissing => ErrorCode::RecoveryRequired,
        ImportSessionErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        ImportSessionErrorCode::CleanupIncomplete | ImportSessionErrorCode::RecoveryRequired => {
            ErrorCode::RecoveryRequired
        }
        ImportSessionErrorCode::SnapshotIo => ErrorCode::Io,
        _ => ErrorCode::InvalidState,
    };
    LocalPoolError::new(code, error.message).into()
}

pub(super) fn default_true() -> bool {
    true
}

use super::quota_refresh::{
    ConfirmAccountImportResponse, ImportItemResult, QUOTA_COMMAND_TIMEOUT_OVERHEAD,
    TOKEN_REFRESH_SKEW_MS,
};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::import_session::{ImportSession, ImportSessionStore};
use crate::local_pool::accounts::proxy::{
    common_proxy_config, effective_proxy_config, ensure_account_proxy,
};
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::{current_time_ms, sync_records_or_rollback};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::ProviderSourceRecord;
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use crate::local_pool::store::secret_store;
use crate::platform::default_codex_home;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;
use zenith_relay_core::accounts::{
    parse_import, ImportAuthMode, ImportIssue, ImportIssueCode, ImportPreview, ImportPreviewStatus,
    ImportQuotaStatus, ParsedImport, ParsedImportItem, MAX_IMPORT_ITEMS,
};
use zenith_relay_core::providers::chatgpt::CodexQuotaClient;
use zenith_relay_core::{
    discover_source_models_and_protocol_bindings, ApiModelPriceOverride, ProviderSource,
    SourceProtocolBinding, WireApi,
};

mod account_import;
mod account_lookup;
mod account_policy;
mod claims;
mod credential_material;
mod documents;
mod errors;
mod identity;
mod persistence;
mod policy;
mod prepared_items;
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
#[cfg(test)]
pub(super) use credential_material::lookup_import_account_id;
pub(in crate::local_pool::accounts) use credential_material::{
    build_import_credential_material, ImportedCredentialMaterial,
};
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
        let original = parsed_item_value(&item, row.auth_mode);
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

pub(super) fn default_true() -> bool {
    true
}

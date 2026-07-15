use super::{
    current_time_ms, restart_or_rollback, sync_accounts_or_rollback, sync_records_or_rollback,
    sync_refreshed_account_or_rollback,
};
use crate::local_pool::{
    accounts::{
        authority::{CredentialPersistence, StoredRefreshAdapter},
        credentials::{
            CredentialError, CredentialErrorCode, CredentialStore, StoredCodexCredentials,
        },
        exports::{
            finish_account_export, normalize_account_ids, AccountExportInput, AccountExportResult,
        },
        import_session::{
            ImportSession, ImportSessionError, ImportSessionErrorCode, ImportSessionStore,
        },
        imports::{
            combine_import_documents, ImportAuthMode, ImportIssue, ImportIssueCode, ImportPreview,
            ImportPreviewStatus, ImportQuotaStatus, ParsedImportItem, MAX_IMPORT_BYTES,
            MAX_IMPORT_ITEMS,
        },
        models::{CodexModelsClient, ModelDiscoveryFailure, ModelDiscoveryFailureCode},
        oauth::{collect_limited, CodexOAuthClient, LimitedBodyError},
        proxy::{common_proxy_config, effective_proxy_config, ensure_account_proxy},
        quota::{CodexQuotaClient, QuotaRefreshOutcome},
        quota_service::{apply_quota_failure, apply_quota_success},
        records, NativeSecretBackend,
    },
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    models::{
        AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot,
        ProviderSourceRecord,
    },
    profiles::codex,
    state::DesktopState,
    store::secret_store,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{TimeZone, Utc};
use reqwest::{header::HeaderValue, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use url::Url;
use uuid::Uuid;
use zenith_relay_core::{
    accounts::{build_account_export, AccountExportCredential, TokenPersistenceAdapter},
    accounts::{AccountAuthMode, AccountAuthState, AccountHealthState, TokenSet},
    automations::AccountSelector,
    discover_source_models,
    protocol::RevealedAccountIdentity,
    quota::{QuotaRefreshFailure, QuotaTransition},
    ProviderSource, ProxyConfig, WireApi,
};

const MAX_ACCOUNT_LABEL_BYTES: usize = 128;
const MAX_MODEL_BYTES: usize = 256;
const MAX_MODELS: usize = 4_096;
const MAX_JWT_BYTES: usize = 64 * 1024;
const MAX_JWT_PAYLOAD_BYTES: usize = 16 * 1024;
const DEFAULT_OPENAI_SOURCE_URL: &str = "https://api.openai.com/v1";
const CODEX_ACCOUNT_CHECK_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/accounts/check";
const MAX_ACCOUNT_PROFILE_RESPONSE_BYTES: usize = 256 * 1024;
const TOKEN_REFRESH_SKEW_MS: u64 = 60_000;
const QUOTA_COMMAND_TIMEOUT_OVERHEAD: Duration = Duration::from_secs(5);
const QUOTA_REFRESH_BATCH_SIZE: usize = 5;
const QUOTA_REFRESH_RETRY_MS: u64 = 60_000;
const QUOTA_REFRESH_LEAD_MS: u64 = 60_000;

type CommandResult<T> = std::result::Result<T, CommandError>;
type ItemResult<T> = std::result::Result<T, ImportItemError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartAccountImportInput {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    documents: Vec<String>,
    #[serde(default)]
    source_file: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareAccountImportInput {
    session_id: String,
    #[serde(default)]
    probe_quota: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmAccountImportInput {
    session_id: String,
    selected_item_ids: Vec<String>,
    #[serde(default)]
    add_to_pool: bool,
    #[serde(default = "default_true")]
    discover_models: bool,
    #[serde(default)]
    probe_quota: bool,
    #[serde(default)]
    models: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateAccountInput {
    account_id: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default)]
    weight: Option<u32>,
    #[serde(default)]
    allowed_models: Option<Vec<String>>,
    #[serde(default)]
    excluded_models: Option<Vec<String>>,
    #[serde(default)]
    in_pool: Option<bool>,
    #[serde(default)]
    draining: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAccountProxyInput {
    account_id: String,
    proxy_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignAccountProxiesInput {
    account_ids: Vec<String>,
    proxy_urls: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyAssignmentResult {
    assigned: usize,
    unused: usize,
}

#[tauri::command]
pub fn reveal_local_account_identity(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<RevealedAccountIdentity> {
    let account_id = normalize_account_ids(vec![account_id])?
        .pop()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "account id is required"))?;
    {
        let store = state.store()?;
        if store.account(&account_id).is_none() {
            return Err(LocalPoolError::new(ErrorCode::NotFound, "account was not found").into());
        }
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend)
        .require(&account_id)
        .map_err(credential_local_error)?;
    let identity = revealable_account_identity(&credentials)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account identity is unavailable"))?
        .to_string();
    Ok(RevealedAccountIdentity {
        account_id,
        identity,
    })
}

fn revealable_account_identity(credentials: &StoredCodexCredentials) -> Option<&str> {
    credentials
        .email()
        .or_else(|| credentials.provider_account_id())
        .or_else(|| credentials.provider_user_id())
}

#[tauri::command]
pub fn export_local_accounts(
    input: AccountExportInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<AccountExportResult> {
    let account_ids = normalize_account_ids(input.account_ids)?;
    let records = {
        let store = state.store()?;
        account_ids
            .iter()
            .map(|account_id| {
                store.account(account_id).cloned().ok_or_else(|| {
                    LocalPoolError::new(ErrorCode::NotFound, "account export item was not found")
                })
            })
            .collect::<LocalResult<Vec<_>>>()?
    };
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let accounts = records
        .iter()
        .map(|record| {
            let credentials = credential_store
                .require(&record.account.id)
                .map_err(credential_local_error)?;
            Ok(AccountExportCredential {
                label: record.account.label.clone(),
                email: credentials.email().map(str::to_string),
                access_token: credentials.access_token().to_string(),
                refresh_token: credentials.refresh_token().map(str::to_string),
                id_token: credentials.id_token().map(str::to_string),
                account_id: credentials.provider_account_id().map(str::to_string),
                user_id: credentials.provider_user_id().map(str::to_string),
                organization_id: credentials.organization_id().map(str::to_string),
                plan_type: record
                    .account
                    .subscription
                    .plan_type
                    .clone()
                    .or_else(|| credentials.plan_type().map(str::to_string)),
                expires_at_ms: credentials.expires_at_ms(),
                issued_at_ms: credentials.issued_at_ms(),
                subscription_active_until_ms: record.account.subscription.active_until_ms,
                created_at_ms: record.account.created_at_ms,
                priority: record.priority,
                enabled: record.account.enabled,
            })
        })
        .collect::<LocalResult<Vec<_>>>()?;
    let document = build_account_export(input.format, &accounts, current_time_ms())
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    finish_account_export(document, input.destination, &app, &state)
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
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: safe_code(code),
            message: message.to_string(),
        }
    }

    fn recovery(message: &str) -> Self {
        Self::new("recovery_required", message)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum AccountQuotaOutcome {
    Skipped,
    Updated { transitions: Vec<QuotaTransition> },
    Failed { code: String, retryable: bool },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemResult {
    pub item_id: String,
    pub status: ImportItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<LocalAccountRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderSourceRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<AccountQuotaOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ImportItemError>,
}

impl ImportItemResult {
    fn account_success(
        item_id: String,
        account: LocalAccountRecord,
        quota: AccountQuotaOutcome,
    ) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Succeeded,
            account: Some(account),
            source: None,
            quota: Some(quota),
            error: None,
        }
    }

    fn source_success(item_id: String, source: ProviderSourceRecord) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Succeeded,
            account: None,
            source: Some(source),
            quota: None,
            error: None,
        }
    }

    fn failure(item_id: String, error: ImportItemError) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Failed,
            account: None,
            source: None,
            quota: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmAccountImportResponse {
    pub session_id: String,
    pub results: Vec<ImportItemResult>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshResponse {
    pub account: LocalAccountRecord,
    pub quota: AccountQuotaOutcome,
}

pub(crate) struct PreparedAccountCredentials {
    tokens: TokenSet,
    provider_account_id: String,
    proxy: Option<ProxyConfig>,
}

impl PreparedAccountCredentials {
    pub(crate) fn tokens(&self) -> &TokenSet {
        &self.tokens
    }

    pub(crate) fn provider_account_id(&self) -> &str {
        &self.provider_account_id
    }

    pub(crate) fn proxy(&self) -> Option<&ProxyConfig> {
        self.proxy.as_ref()
    }
}

impl fmt::Debug for PreparedAccountCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAccountCredentials")
            .field("tokens", &self.tokens)
            .field("provider_account_id", &"[redacted]")
            .field("proxy_configured", &self.proxy.is_some())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountQuotaRefreshStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshItemResult {
    pub account_id: String,
    pub status: AccountQuotaRefreshStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<AccountQuotaRefreshResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
}

#[derive(Clone)]
struct ImportRowContext {
    auth_mode: ImportAuthMode,
    selectable: bool,
    plan: Option<String>,
    subscription_active_until_ms: Option<u64>,
}

#[derive(Default, Deserialize)]
struct ImportedJwtClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ImportedProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<ImportedAuthClaims>,
}

#[derive(Default, Deserialize)]
struct ImportedProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Default, Deserialize)]
struct ImportedAuthClaims {
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_subscription_active_until: Option<serde_json::Value>,
    #[serde(default)]
    chatgpt_user_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    chatgpt_account_is_fedramp: bool,
}

#[derive(Default)]
struct ImportedIdentity {
    email: Option<String>,
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
    provider_user_id: Option<String>,
    provider_account_id: Option<String>,
    account_is_fedramp: bool,
    access_expires_at_ms: Option<u64>,
}

struct ImportedCredentialMaterial {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    email: Option<String>,
    provider_account_id: Option<String>,
    provider_user_id: Option<String>,
    organization_id: Option<String>,
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
    account_is_fedramp: bool,
}

impl ImportedCredentialMaterial {
    fn into_stored(
        self,
        local_account_id: &str,
        issued_at_ms: u64,
        generation: u64,
    ) -> ItemResult<StoredCodexCredentials> {
        StoredCodexCredentials::new(
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
        .map_err(credential_item_error)
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
    let session = ImportSessionStore::new(state.root.clone(), NativeSecretBackend)
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
    let _mutation = state.setup_guard().await;
    let (content, _) = normalize_import_input(StartAccountImportInput {
        content: None,
        documents,
        source_file: None,
    })?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(&state, &credentials)?;
    let sessions = ImportSessionStore::new(state.root.clone(), NativeSecretBackend);
    let session = sessions
        .start(
            &content,
            None,
            &existing.keys().cloned().collect::<Vec<_>>(),
        )
        .map_err(import_session_error)?;
    let session_id = session.session_id.clone();
    let probe_quota = should_probe_import_quota(true, session.preview.rows.len());
    let prepared = async {
        let (content, preview) =
            prepare_import_preview(&state, &credentials, session, probe_quota).await?;
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
        Ok(session) => Ok(Some(session.into())),
        Err(error) => {
            let _ = sessions.cancel(&session_id);
            Err(error)
        }
    }
}

pub(crate) fn pick_account_import_documents(app: &AppHandle) -> CommandResult<Option<Vec<String>>> {
    let Some(files) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
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
            .is_some_and(|value| value.eq_ignore_ascii_case("json"))
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import file must use the .json extension",
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

fn normalize_import_input(
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
    let session = ImportSessionStore::new(state.root.clone(), NativeSecretBackend)
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
    let sessions = ImportSessionStore::new(state.root.clone(), NativeSecretBackend);
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
    ImportSessionStore::new(state.root.clone(), NativeSecretBackend)
        .cancel(&session_id)
        .map_err(import_session_error)?;
    Ok(())
}

#[tauri::command]
pub async fn confirm_local_account_import(
    input: ConfirmAccountImportInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ConfirmAccountImportResponse> {
    let _mutation = state.setup_guard().await;
    confirm_local_account_import_inner(input, &state).await
}

async fn confirm_local_account_import_inner(
    input: ConfirmAccountImportInput,
    state: &DesktopState,
) -> CommandResult<ConfirmAccountImportResponse> {
    let selected_item_ids = normalize_selected_item_ids(input.selected_item_ids)?;
    let configured_models = normalize_models(input.models.clone())?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let sessions = ImportSessionStore::new(state.root.clone(), NativeSecretBackend);
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

    for item_id in selected_item_ids {
        let Some(context) = row_context.get(&item_id) else {
            results.push(ImportItemResult::failure(
                item_id,
                ImportItemError::new("item_not_found", "import item was not found"),
            ));
            continue;
        };
        if !context.selectable {
            results.push(ImportItemResult::failure(
                item_id,
                ImportItemError::new("item_not_selectable", "import item cannot be selected"),
            ));
            continue;
        }
        let Some(item) = items.remove(&item_id) else {
            results.push(ImportItemResult::failure(
                item_id,
                ImportItemError::new(
                    "item_not_selectable",
                    "import item has no usable credentials",
                ),
            ));
            continue;
        };
        if context.auth_mode == ImportAuthMode::ApiKey {
            match import_source_item(
                state,
                item,
                input.add_to_pool,
                input.discover_models,
                &configured_models,
            )
            .await
            {
                Ok(source) => results.push(ImportItemResult::source_success(item_id, source)),
                Err(error) => results.push(ImportItemResult::failure(item_id, error)),
            }
        } else {
            match import_account_item(
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
                    results.push(ImportItemResult::account_success(item_id, account, quota));
                }
                Err(error) => results.push(ImportItemResult::failure(item_id, error)),
            }
        }
    }

    sessions
        .complete(&input.session_id)
        .map_err(import_session_error)?;
    Ok(ConfirmAccountImportResponse {
        session_id: input.session_id,
        results,
    })
}

#[tauri::command]
pub async fn update_local_account(
    input: UpdateAccountInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let account_id = input.account_id.clone();
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    apply_account_patch(&mut account, input)?;
    validate_account_record(&account)?;
    state.mark_quota_refresh(&account_id, current_time_ms())?;
    let (old_accounts, old_keys) = current_account_records(&state)?;
    state.store()?.upsert_account(account)?;
    sync_accounts_or_rollback(&state, old_accounts, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_account_proxy(
    input: SetAccountProxyInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    if state.store()?.account(&input.account_id).is_none() {
        return Err(LocalPoolError::new(ErrorCode::NotFound, "account not found").into());
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let old = credentials
        .require(&input.account_id)
        .map_err(credential_local_error)?;
    let updated = old
        .clone()
        .with_proxy_url(input.proxy_url)
        .map_err(credential_local_error)?;
    if old.proxy_url() == updated.proxy_url() {
        return state.snapshot().await.map_err(Into::into);
    }
    state.mark_quota_refresh(&input.account_id, current_time_ms())?;
    credentials.save(&updated).map_err(credential_local_error)?;
    restart_or_rollback(&state, || {
        credentials.save(&old).map_err(credential_local_error)
    })
    .await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn assign_local_account_proxies(
    input: AssignAccountProxiesInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ProxyAssignmentResult> {
    let _mutation = state.setup_guard().await;
    if input.account_ids.is_empty() || input.proxy_urls.len() < input.account_ids.len() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "proxy list must contain at least one URL per selected account",
        )
        .into());
    }
    let mut seen = HashSet::new();
    if input
        .account_ids
        .iter()
        .any(|account_id| !seen.insert(account_id.clone()))
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account proxy assignment contains duplicate account ids",
        )
        .into());
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let mut updates = Vec::with_capacity(input.account_ids.len());
    for (account_id, proxy_url) in input.account_ids.iter().zip(&input.proxy_urls) {
        if state.store()?.account(account_id).is_none() {
            return Err(LocalPoolError::new(ErrorCode::NotFound, "account not found").into());
        }
        let old = credentials
            .require(account_id)
            .map_err(credential_local_error)?;
        let updated = old
            .clone()
            .with_proxy_url(Some(proxy_url.clone()))
            .map_err(credential_local_error)?;
        updates.push((old, updated));
        state.mark_quota_refresh(account_id, current_time_ms())?;
    }
    for index in 0..updates.len() {
        if let Err(error) = credentials
            .save(&updates[index].1)
            .map_err(credential_local_error)
        {
            restore_proxy_credentials(&credentials, &updates[..index])?;
            return Err(error.into());
        }
    }
    let rollback = updates.clone();
    restart_or_rollback(&state, || {
        restore_proxy_credentials(&credentials, &rollback)
    })
    .await?;
    Ok(ProxyAssignmentResult {
        assigned: updates.len(),
        unused: input.proxy_urls.len().saturating_sub(updates.len()),
    })
}

fn restore_proxy_credentials(
    credentials: &CredentialStore<NativeSecretBackend>,
    updates: &[(StoredCodexCredentials, StoredCodexCredentials)],
) -> LocalResult<()> {
    for (old, _) in updates {
        credentials.save(old).map_err(credential_local_error)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_local_account_enabled(
    account_id: String,
    enabled: bool,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if account.account.enabled == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    account.account.enabled = enabled;
    if enabled {
        validate_account_record(&account)?;
    }
    let (old_accounts, old_keys) = current_account_records(&state)?;
    state.store()?.upsert_account(account)?;
    sync_accounts_or_rollback(&state, old_accounts, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_account_draining(
    account_id: String,
    draining: bool,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if account.account.draining == draining {
        return state.snapshot().await.map_err(Into::into);
    }
    account.account.draining = draining;
    let (old_accounts, old_keys) = current_account_records(&state)?;
    state.store()?.upsert_account(account)?;
    sync_accounts_or_rollback(&state, old_accounts, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn delete_local_account(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let (old_accounts, old_keys, _) = current_account_state(&state)?;
    if !old_accounts
        .iter()
        .any(|account| account.account.id == account_id)
    {
        return Err(LocalPoolError::new(ErrorCode::NotFound, "account not found").into());
    }
    let old_credential = credentials
        .load(&account_id)
        .map_err(credential_local_error)?;
    let old_automations = state.store()?.automations().clone();
    let previous_quota_refresh = state.quota_refresh_snapshot()?;
    let previous_wake = state.wake_snapshot()?;
    let bindings = codex::account_bindings(&state.profile_backup_root())?
        .into_iter()
        .filter(|binding| binding.credential_id == account_id)
        .collect::<Vec<_>>();
    if !bindings.is_empty() && old_credential.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account profile binding exists without stored credentials",
        )
        .into());
    }
    let restored_bindings =
        restore_bound_account_profiles(&state, &bindings, old_credential.as_ref())?;
    if let Err(error) = state.remove_pending_wakes_for_account(&account_id) {
        rollback_deleted_account_side_effects(
            &state,
            &credentials,
            &account_id,
            old_credential.as_ref(),
            previous_quota_refresh,
            previous_wake,
            old_automations,
            &restored_bindings,
            &error,
        )?;
        return Err(error.into());
    }
    let accounts = old_accounts
        .iter()
        .filter(|account| account.account.id != account_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut keys = old_keys.clone();
    prune_key_account_scopes(&mut keys, &accounts);
    let automations = prune_account_task_selectors(old_automations.clone(), &account_id);
    if let Err(error) = credentials
        .delete(&account_id)
        .map_err(credential_local_error)
    {
        rollback_deleted_account_side_effects(
            &state,
            &credentials,
            &account_id,
            old_credential.as_ref(),
            previous_quota_refresh,
            previous_wake,
            old_automations,
            &restored_bindings,
            &error,
        )?;
        return Err(error.into());
    }
    match state.remove_quota_refresh(&account_id) {
        Ok(_) => {}
        Err(error) => {
            rollback_deleted_account_side_effects(
                &state,
                &credentials,
                &account_id,
                old_credential.as_ref(),
                previous_quota_refresh,
                previous_wake,
                old_automations,
                &restored_bindings,
                &error,
            )?;
            return Err(error.into());
        }
    }
    if let Err(error) = state
        .store()?
        .replace_account_state(accounts, keys, automations)
    {
        rollback_deleted_account_side_effects(
            &state,
            &credentials,
            &account_id,
            old_credential.as_ref(),
            previous_quota_refresh,
            previous_wake,
            old_automations,
            &restored_bindings,
            &error,
        )?;
        return Err(error.into());
    }
    if let Err(error) = sync_account_state_or_rollback(
        &state,
        old_accounts.clone(),
        old_keys.clone(),
        old_automations.clone(),
    )
    .await
    {
        rollback_deleted_account_side_effects(
            &state,
            &credentials,
            &account_id,
            old_credential.as_ref(),
            previous_quota_refresh,
            previous_wake,
            old_automations,
            &restored_bindings,
            &error,
        )?;
        repair_gateway_after_credential_restore(&state, old_accounts, old_keys, &error).await?;
        return Err(error.into());
    }
    state.token_authority().remove(&account_id);
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn refresh_local_account_quota(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<AccountQuotaRefreshResponse> {
    refresh_manual_account_quota(&state, &account_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn refresh_all_local_account_quotas(
    state: State<'_, DesktopState>,
) -> CommandResult<Vec<AccountQuotaRefreshItemResult>> {
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    Ok(refresh_account_quotas(&state, account_ids).await)
}

#[tauri::command]
pub async fn refresh_local_pool_account_quotas(
    state: State<'_, DesktopState>,
) -> CommandResult<Vec<AccountQuotaRefreshItemResult>> {
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .filter(|account| account.account.in_pool && account.account.enabled)
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    Ok(refresh_account_quotas(&state, account_ids).await)
}

async fn refresh_account_quotas(
    state: &DesktopState,
    account_ids: Vec<String>,
) -> Vec<AccountQuotaRefreshItemResult> {
    let mut results = Vec::with_capacity(account_ids.len());
    for chunk in account_ids.chunks(QUOTA_REFRESH_BATCH_SIZE) {
        let (first, second, third, fourth, fifth) = tokio::join!(
            refresh_batch_slot(state, chunk.first()),
            refresh_batch_slot(state, chunk.get(1)),
            refresh_batch_slot(state, chunk.get(2)),
            refresh_batch_slot(state, chunk.get(3)),
            refresh_batch_slot(state, chunk.get(4)),
        );
        results.extend([first, second, third, fourth, fifth].into_iter().flatten());
    }
    results
}

async fn refresh_batch_slot(
    state: &DesktopState,
    account_id: Option<&String>,
) -> Option<AccountQuotaRefreshItemResult> {
    let account_id = account_id?.clone();
    let result = refresh_manual_account_quota(state, &account_id).await;
    Some(match result {
        Ok(response) => AccountQuotaRefreshItemResult {
            account_id,
            status: AccountQuotaRefreshStatus::Succeeded,
            response: Some(response),
            error: None,
        },
        Err(error) => AccountQuotaRefreshItemResult {
            account_id,
            status: AccountQuotaRefreshStatus::Failed,
            response: None,
            error: Some(error.into()),
        },
    })
}

async fn refresh_manual_account_quota(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<AccountQuotaRefreshResponse> {
    match refresh_account_quota_once(state, account_id, false).await {
        Ok(response) => {
            settle_manual_quota_refresh(state, account_id, &response)?;
            Ok(response)
        }
        Err(error) => {
            settle_manual_quota_error(state, account_id, &error)?;
            Err(error)
        }
    }
}

pub(crate) async fn sync_managed_account_profile(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<bool> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let stored = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    let provider_account_id = stored
        .provider_account_id()
        .map(str::to_string)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account credentials do not contain a provider account id",
            )
        })?;
    let persistence =
        CredentialPersistence::new(credentials.clone(), state.account_metadata_sink());
    let now_ms = current_time_ms();
    let Some(update) = codex::managed_account_token_update(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        stored.access_token(),
        &provider_account_id,
    )?
    else {
        return Ok(false);
    };
    let identity = imported_identity(update.id_token.as_deref(), Some(&update.access_token));
    if identity
        .provider_account_id
        .as_deref()
        .is_some_and(|value| value != provider_account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed Codex profile token belongs to another account",
        ));
    }
    let tokens = TokenSet::new(
        update.access_token,
        Some(update.refresh_token),
        update.id_token,
        identity.access_expires_at_ms,
        now_ms,
        stored.generation().saturating_add(1),
    )
    .map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "managed Codex profile tokens are invalid",
        )
    })?;
    persistence
        .persist(account_id, &tokens)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!("failed to persist managed Codex tokens: {}", error.code),
            )
        })?;
    persistence
        .persist_auth_state(account_id, AccountAuthState::Active)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!("failed to restore managed Codex auth state: {}", error.code),
            )
        })?;
    state
        .token_authority()
        .register(account_id, tokens.clone(), AccountAuthState::Active)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to register managed Codex tokens: {error}"),
            )
        })?;
    codex::sync_account_bindings(
        &state.profile_backup_root(),
        account_id,
        &tokens,
        &provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        &tokens,
        &provider_account_id,
    )?;
    Ok(true)
}

pub(crate) async fn prepare_account_credentials(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<PreparedAccountCredentials> {
    sync_managed_account_profile(state, account_id).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let initial_account = state
        .store()?
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let stored = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    let gateway = state.store()?.gateway().clone();
    let proxy = effective_proxy_config(&gateway, &stored)?;
    let authority = state.token_authority();
    authority
        .register(
            account_id,
            stored.to_token_set().map_err(credential_local_error)?,
            initial_account.account.auth_state,
        )
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to register account token state: {error}"),
            )
        })?;
    let oauth = Arc::new(
        CodexOAuthClient::new_with_proxy(proxy.as_ref())
            .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?,
    );
    let refresh = StoredRefreshAdapter::new(
        state.root.clone(),
        credentials.clone(),
        oauth,
        TOKEN_REFRESH_SKEW_MS,
    )
    .map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "failed to initialize account refresh locks",
        )
    })?;
    let persistence =
        CredentialPersistence::new(credentials.clone(), state.account_metadata_sink());
    let now_ms = current_time_ms();
    let prepared = authority
        .prepare_and_persist(
            account_id,
            now_ms,
            TOKEN_REFRESH_SKEW_MS,
            &refresh,
            &persistence,
        )
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to prepare account credentials: {error}"),
            )
        })?;
    let current_credentials = credentials
        .require(account_id)
        .map_err(credential_local_error)?;
    let provider_account_id = current_credentials
        .provider_account_id()
        .map(str::to_string)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account credentials do not contain a provider account id",
            )
        })?;
    let proxy = effective_proxy_config(&gateway, &current_credentials)?;
    codex::sync_account_bindings(
        &state.profile_backup_root(),
        account_id,
        &prepared.tokens,
        &provider_account_id,
    )?;
    codex::sync_local_gateway_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        account_id,
        &prepared.tokens,
        &provider_account_id,
    )?;
    Ok(PreparedAccountCredentials {
        tokens: prepared.tokens,
        provider_account_id,
        proxy,
    })
}

pub(crate) async fn refresh_account_quota_once(
    state: &DesktopState,
    account_id: &str,
    force_subscription_refresh: bool,
) -> LocalResult<AccountQuotaRefreshResponse> {
    let prepared = prepare_account_credentials(state, account_id).await?;
    let now_ms = current_time_ms();
    let request_timeout =
        Duration::from_secs(state.store()?.gateway().quota_request_timeout_seconds);
    let mut subscription = state
        .store()?
        .account(account_id)
        .map(|account| account.account.subscription.clone())
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if subscription.active_until_ms.is_none() {
        if let Some(active_until_ms) = imported_identity(
            prepared.tokens().id_token(),
            Some(prepared.tokens().access_token()),
        )
        .subscription_active_until_ms
        {
            subscription = zenith_relay_core::quota::Subscription::normalize(
                zenith_relay_core::quota::SubscriptionInput {
                    plan_type: subscription.plan_type.clone(),
                    active_until_ms: Some(active_until_ms),
                    forbidden: false,
                    observed_at_ms: now_ms,
                },
            );
        }
    }
    let quota = CodexQuotaClient::new_with_proxy_and_timeout(prepared.proxy(), request_timeout)
        .map_err(|failure| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("failed to initialize quota client: {}", failure.code),
            )
        })?;
    let refresh_subscription = force_subscription_refresh
        || zenith_relay_core::quota::subscription_refresh_due(
            subscription.active_until_ms,
            subscription.updated_at_ms,
            now_ms,
        );
    let model_discovery = async {
        match CodexModelsClient::new_with_proxy(prepared.proxy()) {
            Ok(client) => {
                client
                    .discover(
                        prepared.tokens().access_token(),
                        prepared.provider_account_id(),
                        zenith_relay_core::accounts::CODEX_MODELS_CLIENT_VERSION,
                    )
                    .await
            }
            Err(error) => Err(error),
        }
    };
    let (refreshed, discovered_models) = tokio::join!(
        tokio::time::timeout(
            request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
            quota.refresh_quota(
                prepared.tokens().access_token(),
                prepared.provider_account_id(),
                now_ms,
                &subscription,
                refresh_subscription,
            ),
        ),
        model_discovery,
    );

    let _mutation = state.setup_guard().await;
    let (old_accounts, old_keys) = current_account_records(state)?;
    let mut account = state
        .store()?
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let previous_models = account.models.iter().cloned().collect::<BTreeSet<_>>();
    if account.account.subscription.active_until_ms.is_none()
        && subscription.active_until_ms.is_some()
    {
        account.account.subscription = subscription;
    }
    let outcome = match refreshed {
        Ok(outcome) => apply_quota_outcome(&mut account, outcome, now_ms),
        Err(_) => {
            let failure = QuotaRefreshFailure::new("quota_timeout", true);
            apply_quota_failure(&mut account, &failure, now_ms);
            AccountQuotaOutcome::Failed {
                code: failure.code,
                retryable: failure.retryable,
            }
        }
    };
    apply_model_discovery(&mut account, discovered_models);
    let models_changed = account.models.iter().cloned().collect::<BTreeSet<_>>() != previous_models;
    state.store()?.upsert_account(account.clone())?;
    sync_refreshed_account_or_rollback(state, account_id, models_changed, old_accounts, old_keys)
        .await?;
    Ok(AccountQuotaRefreshResponse {
        account,
        quota: outcome,
    })
}

fn settle_manual_quota_refresh(
    state: &DesktopState,
    account_id: &str,
    response: &AccountQuotaRefreshResponse,
) -> LocalResult<()> {
    state.remove_quota_refresh(account_id)?;
    let refresh_interval_seconds = state.store()?.gateway().quota_refresh_interval_seconds;
    if let Some(due_at_ms) =
        next_quota_refresh_at(response, current_time_ms(), refresh_interval_seconds)
    {
        state.mark_quota_refresh(account_id, due_at_ms)?;
    }
    Ok(())
}

fn settle_manual_quota_error(
    state: &DesktopState,
    account_id: &str,
    error: &LocalPoolError,
) -> LocalResult<()> {
    state.remove_quota_refresh(account_id)?;
    if !matches!(&error.code, ErrorCode::NotFound) {
        state.mark_quota_refresh(
            account_id,
            current_time_ms().saturating_add(QUOTA_REFRESH_RETRY_MS),
        )?;
    }
    Ok(())
}

pub(crate) fn next_quota_refresh_at(
    response: &AccountQuotaRefreshResponse,
    now_ms: u64,
    refresh_interval_seconds: u64,
) -> Option<u64> {
    let refresh_interval_ms = refresh_interval_seconds.saturating_mul(1_000);
    match &response.quota {
        AccountQuotaOutcome::Updated { .. } => {
            let fallback = now_ms.saturating_add(refresh_interval_ms);
            let reset_due = response
                .account
                .account
                .quota
                .primary
                .iter()
                .chain(response.account.account.quota.secondary.iter())
                .filter_map(|window| window.reset_at_ms)
                .filter(|reset_at_ms| *reset_at_ms > now_ms)
                .map(|reset_at_ms| {
                    reset_at_ms
                        .saturating_sub(QUOTA_REFRESH_LEAD_MS)
                        .max(now_ms.saturating_add(1_000))
                })
                .min();
            Some(reset_due.map_or(fallback, |due_at_ms| due_at_ms.min(fallback)))
        }
        AccountQuotaOutcome::Failed {
            retryable: true, ..
        } => Some(now_ms.saturating_add(QUOTA_REFRESH_RETRY_MS)),
        AccountQuotaOutcome::Failed {
            retryable: false, ..
        } => None,
        AccountQuotaOutcome::Skipped => Some(now_ms.saturating_add(refresh_interval_ms)),
    }
}

async fn prepare_import_preview(
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
                message: "Codex account identity is missing".into(),
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
                quota.refresh_data_with_subscription(
                    &material.access_token,
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

fn mark_preview_quota_failed(
    row: &mut crate::local_pool::accounts::imports::ImportPreviewRow,
    message: &str,
) {
    row.quota_status = ImportQuotaStatus::Failed;
    row.status = ImportPreviewStatus::QuotaFailed;
    row.error = Some(ImportIssue {
        code: ImportIssueCode::QuotaProbeFailed,
        message: message.into(),
    });
}

fn parsed_item_value(
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
        value.insert(
            "access_token".into(),
            serde_json::Value::String(material.access_token.clone()),
        );
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

fn parsed_item_value_from_material(
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
    value.insert(
        "access_token".into(),
        serde_json::Value::String(material.access_token.clone()),
    );
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

fn insert_optional_string(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        object.insert(key.into(), serde_json::Value::String(value.to_string()));
    }
}

fn masked_account_identity(value: &str) -> String {
    let suffix = value
        .chars()
        .rev()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    if suffix.is_empty() {
        "Account [redacted]".into()
    } else {
        format!("Account ****{suffix}")
    }
}

fn timestamp_from_ms(value: u64) -> Option<String> {
    let value = i64::try_from(value).ok()?;
    Utc.timestamp_millis_opt(value)
        .single()
        .map(|value| value.to_rfc3339())
}

async fn import_source_item(
    state: &DesktopState,
    item: ParsedImportItem,
    add_to_pool: bool,
    discover_models: bool,
    configured_models: &[String],
) -> ItemResult<ProviderSourceRecord> {
    let api_key = item
        .secrets()
        .api_key()
        .map(str::to_string)
        .ok_or_else(|| ImportItemError::new("api_key_missing", "source API key is missing"))?;
    let base_url = imported_source_base_url(&item)?;
    let existing = find_existing_source(state, &base_url, &api_key)?;
    let wire_api = imported_source_wire_api(&item, existing.as_ref())?;
    let source_id = existing
        .as_ref()
        .map(|source| source.id.clone())
        .unwrap_or_else(|| format!("source_{}", Uuid::new_v4().simple()));
    let secret_ref = existing
        .as_ref()
        .map(|source| source.secret_ref.clone())
        .unwrap_or_else(|| format!("source:{source_id}"));
    let requested_models = if configured_models.is_empty() {
        existing
            .as_ref()
            .map(|source| source.models.clone())
            .unwrap_or_default()
    } else {
        configured_models.to_vec()
    };
    let mut runtime_source = ProviderSource {
        id: source_id.clone(),
        name: existing
            .as_ref()
            .map(|source| source.name.clone())
            .unwrap_or_else(|| item.label.trim().to_string()),
        base_url: base_url.clone(),
        api_key: api_key.clone(),
        wire_api,
        models: requested_models,
    };
    runtime_source
        .validate()
        .map_err(|_| ImportItemError::new("source_invalid", "imported source is invalid"))?;
    runtime_source.models = if discover_models {
        discover_source_models(&runtime_source).await.map_err(|_| {
            ImportItemError::new(
                "source_model_discovery_failed",
                "source model discovery failed",
            )
        })?
    } else if !runtime_source.models.is_empty() {
        runtime_source.models.clone()
    } else {
        return Err(ImportItemError::new(
            "models_required",
            "models are required when discovery is disabled",
        ));
    };
    if runtime_source.models.is_empty() {
        return Err(ImportItemError::new(
            "models_empty",
            "source did not expose any configured models",
        ));
    }

    let mut record = imported_source_record(
        &item,
        runtime_source,
        secret_ref,
        existing.as_ref(),
        discover_models.then(|| Utc::now().to_rfc3339()),
    );
    record.in_pool |= add_to_pool;
    persist_imported_source(state, &record, &api_key, existing.as_ref()).await?;
    Ok(record)
}

fn imported_source_record(
    item: &ParsedImportItem,
    runtime_source: ProviderSource,
    secret_ref: String,
    existing: Option<&ProviderSourceRecord>,
    tested_at: Option<String>,
) -> ProviderSourceRecord {
    let tested = tested_at.is_some();
    let mut record = ProviderSourceRecord {
        id: runtime_source.id,
        name: runtime_source.name,
        enabled: existing.as_ref().is_none_or(|source| source.enabled),
        in_pool: existing.as_ref().is_some_and(|source| source.in_pool),
        draining: existing.as_ref().is_some_and(|source| source.draining),
        base_url: runtime_source.base_url,
        secret_ref,
        wire_api: runtime_source.wire_api,
        models: runtime_source.models,
        allowed_models: existing
            .as_ref()
            .map(|source| source.allowed_models.clone())
            .unwrap_or_default(),
        excluded_models: existing
            .as_ref()
            .map(|source| source.excluded_models.clone())
            .unwrap_or_default(),
        priority: existing
            .as_ref()
            .map(|source| source.priority)
            .or(item.priority)
            .unwrap_or_default(),
        weight: existing.as_ref().map_or(1, |source| source.weight),
        last_used_at: existing
            .as_ref()
            .and_then(|source| source.last_used_at.clone()),
        last_test_at: tested_at.or_else(|| {
            existing
                .as_ref()
                .and_then(|source| source.last_test_at.clone())
        }),
        last_test_status: tested.then(|| "ok".to_string()).or_else(|| {
            existing
                .as_ref()
                .and_then(|source| source.last_test_status.clone())
        }),
        last_error: if tested {
            None
        } else {
            existing
                .as_ref()
                .and_then(|source| source.last_error.clone())
        },
    };
    record.normalize();
    record
}

fn imported_source_base_url(item: &ParsedImportItem) -> ItemResult<String> {
    if item.base_url_supplied && item.base_url.is_none() {
        return Err(ImportItemError::new(
            "source_base_url_invalid",
            "source base URL is invalid",
        ));
    }
    canonical_source_base_url(
        item.base_url
            .as_deref()
            .unwrap_or(DEFAULT_OPENAI_SOURCE_URL),
    )
}

fn imported_source_wire_api(
    item: &ParsedImportItem,
    existing: Option<&ProviderSourceRecord>,
) -> ItemResult<WireApi> {
    if item.protocol_supplied && item.protocol.is_none() {
        return Err(ImportItemError::new(
            "source_protocol_invalid",
            "source protocol is invalid",
        ));
    }
    match item.protocol.as_deref() {
        Some("responses") => Ok(WireApi::Responses),
        Some("chat_completions") => Ok(WireApi::ChatCompletions),
        None => Ok(existing.map_or(WireApi::Responses, |source| source.wire_api)),
        _ => Err(ImportItemError::new(
            "source_protocol_invalid",
            "source protocol is invalid",
        )),
    }
}

fn canonical_source_base_url(value: &str) -> ItemResult<String> {
    let mut url = Url::parse(value.trim()).map_err(|_| {
        ImportItemError::new("source_base_url_invalid", "source base URL is invalid")
    })?;
    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn source_identity_key(base_url: &str, api_key: &str) -> ItemResult<String> {
    let base_url = canonical_source_base_url(base_url)?;
    let secret_hash = format!("{:x}", Sha256::digest(api_key.as_bytes()));
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("source\0{base_url}\0{secret_hash}").as_bytes())
    ))
}

fn find_existing_source(
    state: &DesktopState,
    base_url: &str,
    api_key: &str,
) -> ItemResult<Option<ProviderSourceRecord>> {
    let target = source_identity_key(base_url, api_key)?;
    let sources = state
        .store()
        .map_err(|_| ImportItemError::new("source_store_failed", "source store is unavailable"))?
        .sources()
        .to_vec();
    let mut matching = Vec::new();
    for source in sources {
        let Some(secret) = secret_store::load(&source.secret_ref).map_err(|_| {
            ImportItemError::new(
                "source_secret_store_failed",
                "source secret store is unavailable",
            )
        })?
        else {
            continue;
        };
        if source_identity_key(&source.base_url, &secret)? == target {
            matching.push(source);
        }
    }
    match matching.len() {
        0 => Ok(None),
        1 => Ok(matching.pop()),
        _ => Err(ImportItemError::recovery(
            "multiple local sources have the same credential identity",
        )),
    }
}

async fn persist_imported_source(
    state: &DesktopState,
    record: &ProviderSourceRecord,
    api_key: &str,
    existing: Option<&ProviderSourceRecord>,
) -> ItemResult<()> {
    let (old_sources, old_keys) = current_source_records(state)?;
    let old_secret = existing
        .map(|source| {
            secret_store::load(&source.secret_ref).map_err(|_| {
                ImportItemError::new(
                    "source_secret_store_failed",
                    "source secret store is unavailable",
                )
            })
        })
        .transpose()?
        .flatten();
    secret_store::save(&record.secret_ref, api_key).map_err(|_| {
        ImportItemError::new(
            "source_secret_store_failed",
            "failed to save source credentials",
        )
    })?;
    if state
        .store()
        .map_err(|_| ImportItemError::new("source_store_failed", "source store is unavailable"))?
        .upsert_source(record.clone())
        .is_err()
    {
        restore_source_secret(&record.secret_ref, old_secret.as_deref())?;
        return Err(ImportItemError::new(
            "source_store_failed",
            "failed to save source record",
        ));
    }
    if sync_records_or_rollback(state, old_sources, old_keys)
        .await
        .is_err()
    {
        let store = state.store().map_err(|_| {
            ImportItemError::new("source_store_failed", "source store is unavailable")
        })?;
        let rolled_back = match existing {
            Some(previous) => store.source(&record.id) == Some(previous),
            None => store.source(&record.id).is_none(),
        };
        drop(store);
        if rolled_back {
            restore_source_secret(&record.secret_ref, old_secret.as_deref())?;
        }
        return Err(ImportItemError::new(
            "gateway_sync_failed",
            "failed to apply source to the local gateway",
        ));
    }
    Ok(())
}

fn current_source_records(
    state: &DesktopState,
) -> ItemResult<(Vec<ProviderSourceRecord>, Vec<LocalGatewayKeyRecord>)> {
    let store = state
        .store()
        .map_err(|_| ImportItemError::new("source_store_failed", "source store is unavailable"))?;
    Ok((store.sources().to_vec(), store.keys().to_vec()))
}

fn restore_source_secret(secret_ref: &str, previous: Option<&str>) -> ItemResult<()> {
    match previous {
        Some(secret) => secret_store::save(secret_ref, secret),
        None => secret_store::delete(secret_ref),
    }
    .map_err(|_| ImportItemError::recovery("failed to restore previous source credentials"))
}

#[allow(clippy::too_many_arguments)]
async fn import_account_item(
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
    let material = build_import_credential_material(
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
            "Codex account id is missing from imported credentials",
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
            "Codex account id is missing from imported credentials",
        )
    })?;
    let models = if discover_models {
        let client = CodexModelsClient::new_with_proxy(proxy.as_ref()).map_err(model_item_error)?;
        let models = client
            .discover(
                credentials.access_token(),
                provider_account_id,
                zenith_relay_core::accounts::CODEX_MODELS_CLIENT_VERSION,
            )
            .await
            .map_err(model_item_error)?;
        if models.is_empty() {
            return Err(ImportItemError::new(
                "models_empty",
                "Codex account did not expose any supported models",
            ));
        }
        models
    } else if !configured_models.is_empty() {
        configured_models.to_vec()
    } else if let Some(existing) = &existing_account {
        existing.models.clone()
    } else {
        return Err(ImportItemError::new(
            "models_required",
            "models are required when discovery is disabled",
        ));
    };
    let auth_mode = account_auth_mode(context.auth_mode)?;
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
    let quota = if probe_quota {
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

async fn build_import_credential_material(
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

    if let Some(access_token) = secrets.access_token() {
        let material = ImportedCredentialMaterial {
            access_token: access_token.to_string(),
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

    let refresh_token = original_refresh.ok_or_else(|| {
        ImportItemError::new(
            "access_token_missing",
            "Codex account import requires an access or refresh token",
        )
    })?;
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

async fn resolve_import_account_identity(
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

async fn lookup_import_account_id(
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

fn account_id_from_check_response(payload: &serde_json::Value) -> Option<String> {
    if payload.get("accounts").is_none() {
        if let Some(account_id) = account_id_from_profile_record(payload) {
            return Some(account_id);
        }
    }
    let accounts = payload.get("accounts").unwrap_or(payload);
    if let Some(records) = accounts.as_object() {
        if let Some(ordering) = payload
            .get("account_ordering")
            .and_then(|value| value.as_array())
        {
            for key in ordering.iter().filter_map(|value| value.as_str()) {
                if let Some(record) = records.get(key) {
                    if let Some(account_id) = account_id_from_profile_record(record) {
                        return Some(account_id);
                    }
                    if let Some(account_id) = normalized_profile_account_id(key) {
                        return Some(account_id);
                    }
                }
            }
        }
        for (key, record) in records {
            if let Some(account_id) = account_id_from_profile_record(record) {
                return Some(account_id);
            }
            if let Some(account_id) = normalized_profile_account_id(key) {
                return Some(account_id);
            }
        }
    }
    accounts
        .as_array()?
        .iter()
        .find_map(account_id_from_profile_record)
}

fn account_id_from_profile_record(record: &serde_json::Value) -> Option<String> {
    let record = record
        .get("account")
        .filter(|value| value.is_object())
        .unwrap_or(record);
    ["id", "account_id", "chatgpt_account_id", "workspace_id"]
        .into_iter()
        .find_map(|key| {
            record
                .get(key)
                .and_then(|value| value.as_str())
                .and_then(normalized_profile_account_id)
        })
}

fn normalized_profile_account_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control))
        .then(|| value.to_string())
}

fn hinted_import_proxy(
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

async fn probe_import_quota(
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
        client.refresh_quota(
            credentials.access_token(),
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

fn apply_quota_outcome(
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
        QuotaRefreshOutcome::Failed { failure, .. } => {
            apply_quota_failure(account, &failure, now_ms);
            AccountQuotaOutcome::Failed {
                code: failure.code,
                retryable: failure.retryable,
            }
        }
    }
}

fn apply_model_discovery(
    account: &mut LocalAccountRecord,
    result: std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) {
    match result {
        Ok(models) if !models.is_empty() => {
            account.models = models;
            account.normalize();
            if account
                .account
                .last_error_code
                .as_deref()
                .is_some_and(|code| code.starts_with("models_"))
            {
                account.account.last_error_code = None;
            }
        }
        Ok(_) if account.models.is_empty() => {
            apply_model_discovery_failure(account, "models_empty", false)
        }
        Err(error) if account.models.is_empty() => {
            apply_model_discovery_failure(account, model_failure_code(&error), error.retryable)
        }
        Ok(_) | Err(_) => {}
    }
}

fn apply_model_discovery_failure(account: &mut LocalAccountRecord, code: &str, retryable: bool) {
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

async fn persist_imported_account(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    credentials: &StoredCodexCredentials,
    old_credential: Option<&StoredCodexCredentials>,
    account: LocalAccountRecord,
) -> ItemResult<()> {
    let (old_accounts, old_keys) = current_account_records(state).map_err(|_| {
        ImportItemError::new("account_store_failed", "account store is unavailable")
    })?;
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
    if sync_accounts_or_rollback(state, old_accounts.clone(), old_keys.clone())
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
    if state
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
        .mark_quota_refresh(credentials.local_account_id(), current_time_ms())
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

async fn rollback_after_authority_failure(
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

async fn restore_authority(
    state: &DesktopState,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    old_accounts: &[LocalAccountRecord],
) -> ItemResult<()> {
    let Some(credentials) = old_credential else {
        state.token_authority().remove(account_id);
        return Ok(());
    };
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

fn ensure_account_import_item(item: &ParsedImportItem) -> ItemResult<()> {
    if item.secrets().api_key().is_some() {
        Err(ImportItemError::new(
            "use_source_import",
            "API keys must be imported as compatible API sources",
        ))
    } else {
        Ok(())
    }
}

fn account_auth_mode(mode: ImportAuthMode) -> ItemResult<AccountAuthMode> {
    match mode {
        ImportAuthMode::OAuth => Ok(AccountAuthMode::OAuth),
        ImportAuthMode::ImportedToken => Ok(AccountAuthMode::ImportedToken),
        ImportAuthMode::ApiKey => Err(ImportItemError::new(
            "use_source_import",
            "API keys must be imported as compatible API sources",
        )),
        ImportAuthMode::Unknown => Err(ImportItemError::new(
            "unknown_auth_mode",
            "imported account authentication mode is unknown",
        )),
    }
}

fn merge_existing_account(account: &mut LocalAccountRecord, existing: Option<&LocalAccountRecord>) {
    let Some(existing) = existing else {
        return;
    };
    account.account.label = existing.account.label.clone();
    account.account.tags = existing.account.tags.clone();
    account.account.enabled = existing.account.enabled;
    account.account.in_pool = existing.account.in_pool;
    account.account.draining = existing.account.draining;
    account.account.created_at_ms = existing.account.created_at_ms;
    account.account.last_used_at_ms = existing.account.last_used_at_ms;
    account.account.health = existing.account.health;
    account.account.quota = existing.account.quota.clone();
    account.account.subscription = existing.account.subscription.clone();
    account.account.last_error_code = existing.account.last_error_code.clone();
    account.allowed_models = existing.allowed_models.clone();
    account.excluded_models = existing.excluded_models.clone();
    account.priority = existing.priority;
    account.weight = existing.weight;
}

fn apply_account_patch(
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
    account.normalize();
    Ok(())
}

fn validate_account_record(account: &LocalAccountRecord) -> LocalResult<()> {
    validate_label(&account.account.label)?;
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
    credentials.to_token_set().map_err(credential_local_error)?;
    Ok(())
}

fn account_model_state_is_valid(account: &LocalAccountRecord) -> bool {
    !account.models.is_empty()
        || (account.account.last_error_code.is_some()
            && account.account.health != zenith_relay_core::accounts::AccountHealthState::Healthy)
}

fn validate_label(label: &str) -> LocalResult<()> {
    let label = label.trim();
    if label.is_empty()
        || label.len() > MAX_ACCOUNT_LABEL_BYTES
        || label.chars().any(char::is_control)
    {
        Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account label is invalid",
        ))
    } else {
        Ok(())
    }
}

fn normalize_models(models: Vec<String>) -> LocalResult<Vec<String>> {
    if models.len() > MAX_MODELS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "model list exceeds the supported limit",
        ));
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if model.len() > MAX_MODEL_BYTES || model.chars().any(char::is_control) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "model name is invalid",
            ));
        }
        if seen.insert(model.to_string()) {
            normalized.push(model.to_string());
        }
    }
    Ok(normalized)
}

fn normalize_selected_item_ids(item_ids: Vec<String>) -> CommandResult<Vec<String>> {
    if item_ids.len() > MAX_IMPORT_ITEMS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "selected import item count exceeds the supported limit",
        )
        .into());
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for item_id in item_ids {
        let item_id = item_id.trim();
        let Some(suffix) = item_id.strip_prefix("import_") else {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import item id is invalid",
            )
            .into());
        };
        if suffix.len() != 16 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import item id is invalid",
            )
            .into());
        }
        if seen.insert(item_id.to_string()) {
            normalized.push(item_id.to_string());
        }
    }
    Ok(normalized)
}

fn should_probe_import_quota(requested: bool, row_count: usize) -> bool {
    requested && row_count <= QUOTA_REFRESH_BATCH_SIZE
}

fn existing_identity_index(
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

fn provider_identity_key(
    provider_account_id: &str,
    provider_user_id: Option<&str>,
    email: Option<&str>,
) -> String {
    let account = provider_account_id.trim().to_ascii_lowercase();
    let user = provider_user_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let email = email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let identity = match (email, user) {
        (Some(email), _) => format!("account:{account}:email:{email}"),
        (None, Some(user)) => format!("account:{account}:user:{user}"),
        (None, None) => format!("account:{account}"),
    };
    format!(
        "{:x}",
        Sha256::digest(format!("account:{identity}").as_bytes())
    )
}

fn find_existing_account(
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
            "multiple local accounts have the same Codex identity",
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
            "multiple local accounts have the same Codex identity",
        ));
    }
    Ok(matching.pop())
}

fn prune_key_account_scopes(keys: &mut [LocalGatewayKeyRecord], accounts: &[LocalAccountRecord]) {
    let valid_ids = accounts
        .iter()
        .map(|account| account.account.id.as_str())
        .collect::<HashSet<_>>();
    for key in keys {
        if let Some(account_ids) = &mut key.account_ids {
            account_ids.retain(|id| valid_ids.contains(id.as_str()));
        }
    }
}

fn imported_identity(id_token: Option<&str>, access_token: Option<&str>) -> ImportedIdentity {
    let id_claims = id_token.and_then(decode_imported_jwt);
    let access_claims = access_token.and_then(decode_imported_jwt);
    let id_auth = id_claims.as_ref().and_then(|claims| claims.auth.as_ref());
    let access_auth = access_claims
        .as_ref()
        .and_then(|claims| claims.auth.as_ref());
    ImportedIdentity {
        email: claim_email(id_claims.as_ref()).or_else(|| claim_email(access_claims.as_ref())),
        plan_type: auth_string(id_auth, |auth| &auth.chatgpt_plan_type)
            .or_else(|| auth_string(access_auth, |auth| &auth.chatgpt_plan_type)),
        subscription_active_until_ms: id_auth
            .and_then(|auth| auth.chatgpt_subscription_active_until.as_ref())
            .and_then(parse_subscription_timestamp_value_ms)
            .or_else(|| {
                access_auth
                    .and_then(|auth| auth.chatgpt_subscription_active_until.as_ref())
                    .and_then(parse_subscription_timestamp_value_ms)
            }),
        provider_user_id: auth_string(id_auth, |auth| &auth.chatgpt_user_id)
            .or_else(|| auth_string(id_auth, |auth| &auth.user_id))
            .or_else(|| auth_string(access_auth, |auth| &auth.chatgpt_user_id))
            .or_else(|| auth_string(access_auth, |auth| &auth.user_id)),
        provider_account_id: auth_string(id_auth, |auth| &auth.chatgpt_account_id)
            .or_else(|| auth_string(id_auth, |auth| &auth.account_id))
            .or_else(|| auth_string(access_auth, |auth| &auth.chatgpt_account_id))
            .or_else(|| auth_string(access_auth, |auth| &auth.account_id)),
        account_is_fedramp: id_auth
            .or(access_auth)
            .is_some_and(|auth| auth.chatgpt_account_is_fedramp),
        access_expires_at_ms: access_claims
            .and_then(|claims| claims.exp)
            .map(|seconds| seconds.saturating_mul(1_000)),
    }
}

fn parse_subscription_timestamp_value_ms(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(value) => value.as_u64().and_then(normalize_epoch_timestamp_ms),
        serde_json::Value::String(value) => parse_subscription_timestamp_ms(value),
        _ => None,
    }
}

fn parse_subscription_timestamp_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .and_then(normalize_epoch_timestamp_ms);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

fn normalize_epoch_timestamp_ms(value: u64) -> Option<u64> {
    let value = if value < 100_000_000_000 {
        value.checked_mul(1_000)?
    } else {
        value
    };
    i64::try_from(value)
        .ok()
        .and_then(|value| Utc.timestamp_millis_opt(value).single())
        .map(|_| value)
}

fn decode_imported_jwt(token: &str) -> Option<ImportedJwtClaims> {
    if token.is_empty() || token.len() > MAX_JWT_BYTES {
        return None;
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    if decoded.len() > MAX_JWT_PAYLOAD_BYTES {
        return None;
    }
    serde_json::from_slice(&decoded).ok()
}

fn claim_email(claims: Option<&ImportedJwtClaims>) -> Option<String> {
    claims.and_then(|claims| {
        nonempty(claims.email.clone()).or_else(|| {
            claims
                .profile
                .as_ref()
                .and_then(|profile| nonempty(profile.email.clone()))
        })
    })
}

fn auth_string(
    auth: Option<&ImportedAuthClaims>,
    select: impl for<'a> Fn(&'a ImportedAuthClaims) -> &'a Option<String>,
) -> Option<String> {
    auth.and_then(|auth| nonempty(select(auth).clone()))
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn current_account_records(
    state: &DesktopState,
) -> LocalResult<(Vec<LocalAccountRecord>, Vec<LocalGatewayKeyRecord>)> {
    let store = state.store()?;
    Ok((store.accounts().to_vec(), store.keys().to_vec()))
}

fn current_account_state(
    state: &DesktopState,
) -> LocalResult<(
    Vec<LocalAccountRecord>,
    Vec<LocalGatewayKeyRecord>,
    AutomationRecords,
)> {
    let store = state.store()?;
    Ok((
        store.accounts().to_vec(),
        store.keys().to_vec(),
        store.automations().clone(),
    ))
}

async fn sync_account_state_or_rollback(
    state: &DesktopState,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
    old_automations: AutomationRecords,
) -> LocalResult<()> {
    super::restart_or_rollback(state, || {
        state
            .store()?
            .replace_account_state(old_accounts, old_keys, old_automations)
    })
    .await
}

fn prune_account_task_selectors(
    mut automations: AutomationRecords,
    account_id: &str,
) -> AutomationRecords {
    let now_ms = current_time_ms();
    automations.tasks.retain_mut(|task| {
        let AccountSelector::AccountIds(account_ids) = &mut task.account_selector else {
            return true;
        };
        if !account_ids.remove(account_id) {
            return true;
        }
        task.updated_at_ms = now_ms;
        !account_ids.is_empty()
    });
    automations
}

fn restore_credential_item(
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
) -> ItemResult<()> {
    match old_credential {
        Some(credentials) => credential_store.save(credentials),
        None => credential_store.delete(account_id),
    }
    .map_err(|_| ImportItemError::recovery("failed to restore previous account credentials"))
}

fn restore_credential_local(
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    let restored = match old_credential {
        Some(credentials) => credential_store.save(credentials),
        None => Ok(()),
    };
    restored.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; failed to restore previous account credentials",
                cause.message
            ),
        )
    })?;
    if old_credential.is_none() {
        let _ = account_id;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rollback_deleted_account_side_effects(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    previous_quota_refresh: zenith_relay_core::quota::QuotaRefreshQueue,
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    restored_bindings: &[codex::ProfileBinding],
    cause: &LocalPoolError,
) -> LocalResult<()> {
    restore_credential_local(credential_store, account_id, old_credential, cause)?;
    state
        .restore_quota_refresh(previous_quota_refresh)
        .map_err(|error| recovery_after_delete(cause, "quota schedule", error))?;
    state
        .restore_wake(previous_wake, old_automations)
        .map_err(|error| recovery_after_delete(cause, "wake state", error))?;
    reattach_account_profiles(state, restored_bindings, old_credential, cause)?;
    Ok(())
}

fn restore_bound_account_profiles(
    state: &DesktopState,
    bindings: &[codex::ProfileBinding],
    credentials: Option<&StoredCodexCredentials>,
) -> LocalResult<Vec<codex::ProfileBinding>> {
    let mut restored = Vec::with_capacity(bindings.len());
    for binding in bindings {
        match codex::restore_account_profile(
            Path::new(&binding.profile_dir),
            &state.profile_backup_root(),
        ) {
            Ok(Some(binding)) => restored.push(binding),
            Ok(None) => {
                let error = LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "account profile binding disappeared during deletion",
                );
                reattach_account_profiles(state, &restored, credentials, &error)?;
                return Err(error);
            }
            Err(error) => {
                reattach_account_profiles(state, &restored, credentials, &error)?;
                return Err(error);
            }
        }
    }
    Ok(restored)
}

fn reattach_account_profiles(
    state: &DesktopState,
    bindings: &[codex::ProfileBinding],
    credentials: Option<&StoredCodexCredentials>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    if bindings.is_empty() {
        return Ok(());
    }
    let credentials = credentials.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{}; account profile credentials are missing", cause.message),
        )
    })?;
    let tokens = credentials.to_token_set().map_err(|_| {
        LocalPoolError::new(ErrorCode::RecoveryRequired, "account tokens are invalid")
    })?;
    let provider_account_id = credentials.provider_account_id().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account provider identity is missing",
        )
    })?;
    for binding in bindings {
        codex::attach_account(
            Path::new(&binding.profile_dir),
            &state.profile_backup_root(),
            &binding.credential_id,
            &tokens,
            provider_account_id,
        )
        .map_err(|error| recovery_after_delete(cause, "profile binding", error))?;
    }
    Ok(())
}

fn recovery_after_delete(
    cause: &LocalPoolError,
    state: &str,
    error: LocalPoolError,
) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        format!(
            "{}; failed to restore account {state}: {}",
            cause.message, error.message
        ),
    )
}

async fn repair_gateway_after_item_restore(
    state: &DesktopState,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> ItemResult<()> {
    sync_accounts_or_rollback(state, old_accounts, old_keys)
        .await
        .map_err(|_| {
            ImportItemError::recovery("failed to rebuild gateway after credential rollback")
        })
}

async fn repair_gateway_after_credential_restore(
    state: &DesktopState,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    sync_accounts_or_rollback(state, old_accounts, old_keys)
        .await
        .map_err(|repair| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "{}; failed to rebuild gateway after credential rollback: {}",
                    cause.message, repair.message
                ),
            )
        })
}

fn credential_item_error(error: CredentialError) -> ImportItemError {
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

fn import_item_command_error(error: ImportItemError) -> CommandError {
    let code = if error.code == "recovery_required" {
        ErrorCode::RecoveryRequired
    } else {
        ErrorCode::InvalidState
    };
    LocalPoolError::new(code, error.message).into()
}

fn credential_local_error(error: CredentialError) -> LocalPoolError {
    let code = match error.code {
        CredentialErrorCode::SecretMissing => ErrorCode::NotFound,
        CredentialErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        _ => ErrorCode::InvalidState,
    };
    LocalPoolError::new(code, error.message)
}

fn proxy_item_error(error: LocalPoolError) -> ImportItemError {
    ImportItemError::new("proxy_unavailable", &error.message)
}

fn model_item_error(
    error: crate::local_pool::accounts::models::ModelDiscoveryFailure,
) -> ImportItemError {
    ImportItemError::new(model_failure_code(&error), &error.to_string())
}

fn model_failure_code(error: &ModelDiscoveryFailure) -> &'static str {
    match error.code {
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

fn import_session_error(error: ImportSessionError) -> CommandError {
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

fn safe_code(value: &str) -> String {
    let value = value.trim();
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        value.to_ascii_lowercase()
    } else {
        "operation_failed".to_string()
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::imports::parse_import;
    use axum::{routing::get, Json, Router};
    use std::collections::BTreeSet;
    use tokio::net::TcpListener;
    use zenith_relay_core::{
        automations::{
            AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTask, WakeTrigger,
        },
        quota::{QuotaWindow, QuotaWindowKind},
        WireApi,
    };

    fn account_record(account_id: &str) -> LocalAccountRecord {
        let credentials = StoredCodexCredentials::new(
            account_id,
            "access-private".into(),
            Some("refresh-private".into()),
            None,
            None,
            1,
            0,
            None,
            Some("provider-private".into()),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        records::new_account_record(
            &credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            1,
        )
        .unwrap()
    }

    fn wake_task(id: &str, account_ids: &[&str]) -> WakeTask {
        WakeTask {
            id: id.into(),
            name: id.into(),
            enabled: true,
            account_selector: AccountSelector::AccountIds(
                account_ids.iter().map(|id| (*id).to_string()).collect(),
            ),
            window_kinds: BTreeSet::from([QuotaWindowKind::Primary]),
            model_policy: WakeModelPolicy::LightestSupported,
            trigger: WakeTrigger::QuotaFull,
            fallback_schedule: None,
            execution_policy: WakeExecutionPolicy::Automatic,
            jitter_seconds: 0,
            max_attempts_per_cycle: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn revealable_identity_prefers_email_and_falls_back_to_provider_account() {
        let with_email = StoredCodexCredentials::new(
            "account_email",
            "access-private".into(),
            None,
            None,
            None,
            1,
            0,
            Some("private@example.test".into()),
            Some("provider-account".into()),
            Some("provider-user".into()),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            revealable_account_identity(&with_email),
            Some("private@example.test")
        );

        let without_email = StoredCodexCredentials::new(
            "account_provider",
            "access-private".into(),
            None,
            None,
            None,
            1,
            0,
            None,
            Some("provider-account".into()),
            Some("provider-user".into()),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            revealable_account_identity(&without_email),
            Some("provider-account")
        );
    }

    #[test]
    fn selected_ids_are_validated_and_deduplicated_in_order() {
        let selected = normalize_selected_item_ids(vec![
            "import_0123456789abcdef".into(),
            " import_0123456789abcdef ".into(),
            "import_fedcba9876543210".into(),
        ])
        .unwrap();
        assert_eq!(
            selected,
            [
                "import_0123456789abcdef".to_string(),
                "import_fedcba9876543210".to_string()
            ]
        );
        assert!(normalize_selected_item_ids(vec!["../secret".into()]).is_err());
    }

    #[test]
    fn large_import_preview_defers_quota_network_calls() {
        assert!(should_probe_import_quota(true, QUOTA_REFRESH_BATCH_SIZE));
        assert!(!should_probe_import_quota(
            true,
            QUOTA_REFRESH_BATCH_SIZE + 1
        ));
        assert!(!should_probe_import_quota(false, 1));
    }

    #[test]
    fn model_refresh_accepts_unknown_slugs_and_preserves_last_good_list() {
        let mut account = account_record("account_models");
        apply_model_discovery(&mut account, Ok(vec!["gpt-future-codex".into()]));
        assert_eq!(account.models, ["gpt-future-codex"]);

        apply_model_discovery(
            &mut account,
            Err(ModelDiscoveryFailure {
                code: ModelDiscoveryFailureCode::Transport,
                retryable: true,
                http_status: None,
            }),
        );
        assert_eq!(account.models, ["gpt-future-codex"]);
        assert!(account.account.last_error_code.is_none());

        account.models.clear();
        apply_model_discovery(
            &mut account,
            Err(ModelDiscoveryFailure {
                code: ModelDiscoveryFailureCode::Transport,
                retryable: true,
                http_status: None,
            }),
        );
        assert_eq!(
            account.account.last_error_code.as_deref(),
            Some("models_transport")
        );
        assert_eq!(account.account.health, AccountHealthState::Degraded);
    }

    #[test]
    fn selected_import_files_are_read_and_combined_only_in_rust() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-files-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = (1..=3)
            .map(|index| {
                let path = root.join(format!("account-{index}.json"));
                std::fs::write(
                    &path,
                    serde_json::json!({
                        "account_id": format!("provider-{index}"),
                        "access_token": format!("synthetic-access-{index}")
                    })
                    .to_string(),
                )
                .unwrap();
                path
            })
            .collect::<Vec<_>>();

        let documents = read_import_documents(paths).unwrap();
        let combined = combine_import_documents(&documents).unwrap();
        let parsed = parse_import(&combined, None, &[]).unwrap();

        assert_eq!(parsed.items.len(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dropped_import_rejects_non_json_files() {
        let path = std::env::temp_dir().join(format!(
            "zenith-relay-import-{}.txt",
            Uuid::new_v4().simple()
        ));
        std::fs::write(&path, "{}").unwrap();
        assert!(read_import_documents(vec![path.clone()]).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn batch_confirm_persists_every_selected_account_and_credential() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-batch-import-{}",
            Uuid::new_v4().simple()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        let documents = (1..=3)
            .map(|index| {
                serde_json::json!({
                    "name": format!("Imported {index}"),
                    "credentials": {
                        "access_token": format!("synthetic-access-{index}"),
                        "refresh_token": format!("synthetic-refresh-{index}"),
                        "chatgpt_account_id": format!("synthetic-provider-{index}"),
                        "email": format!("member-{index}@example.test"),
                        "subscription_expires_at": format!("2026-08-0{index}T00:00:00Z")
                    }
                })
                .to_string()
            })
            .collect::<Vec<_>>();
        let (content, _) = normalize_import_input(StartAccountImportInput {
            content: None,
            documents,
            source_file: None,
        })
        .unwrap();
        let sessions = ImportSessionStore::new(root.clone(), NativeSecretBackend);
        let session = sessions.start(&content, None, &[]).unwrap();
        let selected_item_ids = session
            .preview
            .rows
            .iter()
            .map(|row| row.item_id.clone())
            .collect::<Vec<_>>();

        let response = confirm_local_account_import_inner(
            ConfirmAccountImportInput {
                session_id: session.session_id,
                selected_item_ids,
                add_to_pool: true,
                discover_models: false,
                probe_quota: false,
                models: vec!["gpt-test".into()],
            },
            &state,
        )
        .await
        .unwrap();

        assert_eq!(response.results.len(), 3);
        assert!(response
            .results
            .iter()
            .all(|result| result.status == ImportItemStatus::Succeeded));
        let accounts = state.store().unwrap().accounts().to_vec();
        assert_eq!(accounts.len(), 3);
        assert_eq!(
            accounts
                .iter()
                .map(|account| account.account.id.as_str())
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let mut provider_ids = HashSet::new();
        for account in &accounts {
            let credentials = credential_store.require(&account.account.id).unwrap();
            provider_ids.insert(credentials.provider_account_id().unwrap().to_string());
            credential_store.delete(&account.account.id).unwrap();
        }
        assert_eq!(provider_ids.len(), 3);
        assert!(accounts.iter().all(|account| account
            .account
            .subscription
            .active_until_ms
            .is_some()));
        assert!(accounts.iter().all(|account| account.account.in_pool));

        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cockpit_api_keys_do_not_require_oauth_quota_preview() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-cockpit-source-import-{}",
            Uuid::new_v4().simple()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        let content = r#"[
            {"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-key-one","api_base_url":"https://one.example.test/v1","api_provider_name":"One API"},
            {"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-key-two","api_base_url":"https://two.example.test/v1","api_provider_name":"Two API"}
        ]"#;
        let sessions = ImportSessionStore::new(root.clone(), NativeSecretBackend);
        let session = sessions.start(content, None, &[]).unwrap();
        let selected_item_ids = session
            .preview
            .rows
            .iter()
            .map(|row| row.item_id.clone())
            .collect();

        let response = confirm_local_account_import_inner(
            ConfirmAccountImportInput {
                session_id: session.session_id,
                selected_item_ids,
                add_to_pool: true,
                discover_models: false,
                probe_quota: true,
                models: vec!["gpt-test".into()],
            },
            &state,
        )
        .await
        .unwrap();

        assert_eq!(response.results.len(), 2);
        assert!(response
            .results
            .iter()
            .all(|result| result.status == ImportItemStatus::Succeeded));
        let sources = state.store().unwrap().sources().to_vec();
        assert_eq!(sources.len(), 2);
        assert!(sources.iter().all(|source| source.in_pool));
        assert_eq!(
            sources
                .iter()
                .map(|source| source.name.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["One API", "Two API"])
        );
        for source in sources {
            secret_store::delete(&source.secret_ref).unwrap();
        }

        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn api_key_auth_json_builds_a_safe_default_responses_source() {
        let mut parsed = parse_import(
            r#"{"auth_mode":"api_key","OPENAI_API_KEY":"sk-private"}"#,
            None,
            &[],
        )
        .unwrap();
        let item = parsed.items.remove(0);
        let base_url = imported_source_base_url(&item).unwrap();
        let wire_api = imported_source_wire_api(&item, None).unwrap();
        let runtime = ProviderSource {
            id: "source_test".into(),
            name: item.label.clone(),
            base_url: base_url.clone(),
            api_key: item.secrets().api_key().unwrap().to_string(),
            wire_api,
            models: vec!["gpt-test".into()],
        };
        runtime.validate().unwrap();
        let source =
            imported_source_record(&item, runtime, "source:source_test".into(), None, None);
        let serialized = serde_json::to_string(&ImportItemResult::source_success(
            item.item_id,
            source.clone(),
        ))
        .unwrap();

        assert_eq!(source.base_url, DEFAULT_OPENAI_SOURCE_URL);
        assert_eq!(source.wire_api, WireApi::Responses);
        assert_eq!(source.models, ["gpt-test"]);
        assert!(!serialized.contains("sk-private"));
        assert!(serialized.contains("source"));
    }

    #[test]
    fn source_duplicate_identity_updates_the_existing_local_record() {
        let mut parsed = parse_import(
            r#"{"api_key":"sk-private","base_url":"https://api.example.test/v1/"}"#,
            None,
            &[],
        )
        .unwrap();
        let item = parsed.items.remove(0);
        let existing = ProviderSourceRecord {
            id: "source_existing".into(),
            name: "Custom name".into(),
            enabled: false,
            in_pool: true,
            draining: true,
            base_url: "https://api.example.test/v1".into(),
            secret_ref: "source:source_existing".into(),
            wire_api: WireApi::ChatCompletions,
            models: vec!["old-model".into()],
            allowed_models: vec!["gpt-*".into()],
            excluded_models: vec!["gpt-old".into()],
            priority: 7,
            weight: 3,
            last_used_at: Some("2026-07-10T00:00:00Z".into()),
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        };
        assert_eq!(
            source_identity_key(&existing.base_url, "sk-private").unwrap(),
            source_identity_key(item.base_url.as_deref().unwrap(), "sk-private").unwrap()
        );
        let wire_api = imported_source_wire_api(&item, Some(&existing)).unwrap();
        let runtime = ProviderSource {
            id: existing.id.clone(),
            name: existing.name.clone(),
            base_url: imported_source_base_url(&item).unwrap(),
            api_key: "sk-private".into(),
            wire_api,
            models: vec!["new-model".into()],
        };
        let updated = imported_source_record(
            &item,
            runtime,
            existing.secret_ref.clone(),
            Some(&existing),
            None,
        );

        assert_eq!(updated.id, existing.id);
        assert_eq!(updated.name, existing.name);
        assert_eq!(updated.wire_api, WireApi::ChatCompletions);
        assert_eq!(updated.models, ["new-model"]);
        assert_eq!(updated.allowed_models, existing.allowed_models);
        assert_eq!(updated.excluded_models, existing.excluded_models);
        assert_eq!(updated.priority, 7);
        assert_eq!(updated.weight, 3);
        assert!(!updated.enabled);
        assert!(updated.draining);
    }

    #[test]
    fn refresh_only_without_explicit_account_id_updates_after_exchange_identity() {
        let parsed = parse_import(r#"{"refresh_token":"refresh-rotated"}"#, None, &[]).unwrap();
        assert!(parsed.items[0].account_id.is_none());
        let mut existing = account_record("account_existing");
        existing.account.label = "My account".into();
        existing.account.token_generation = 7;
        existing.account.in_pool = true;
        existing.priority = 9;
        let resolved = existing.clone();
        let credentials = ImportedCredentialMaterial {
            access_token: "access-rotated".into(),
            refresh_token: Some("refresh-rotated".into()),
            id_token: None,
            expires_at_ms: Some(60_000),
            email: None,
            provider_account_id: Some("provider-private".into()),
            provider_user_id: None,
            organization_id: None,
            plan_type: None,
            subscription_active_until_ms: None,
            account_is_fedramp: false,
        }
        .into_stored(&resolved.account.id, 2, 8)
        .unwrap();
        let mut updated = records::new_account_record(
            &credentials,
            AccountAuthMode::ImportedToken,
            vec!["gpt-test".into()],
            0,
            2,
        )
        .unwrap();
        merge_existing_account(&mut updated, Some(&resolved));

        assert_eq!(credentials.local_account_id(), "account_existing");
        assert_eq!(credentials.generation(), 8);
        assert_eq!(updated.account.id, "account_existing");
        assert_eq!(updated.account.label, "My account");
        assert_eq!(updated.account.token_generation, 8);
        assert!(updated.account.in_pool);
        assert_eq!(updated.priority, 9);
        assert_ne!(
            updated.account.identity.stable_index,
            existing.account.identity.stable_index
        );
    }

    #[test]
    fn provider_identity_hash_matches_import_parser_without_exposing_id() {
        let parsed = parse_import(
            r#"{"account_id":"Provider-Private","chatgpt_user_id":"User-Private","email":"private@example.test","access_token":"access-private"}"#,
            None,
            &[],
        )
        .unwrap();
        let key = provider_identity_key(
            "Provider-Private",
            Some("User-Private"),
            Some("private@example.test"),
        );
        assert_eq!(parsed.items[0].identity_key, key);
        assert!(!key.contains("provider"));
    }

    #[test]
    fn account_patch_normalizes_metadata_and_rejects_zero_weight() {
        let credentials = StoredCodexCredentials::new(
            "account_local",
            "access-private".into(),
            Some("refresh-private".into()),
            None,
            None,
            1,
            0,
            None,
            Some("provider-private".into()),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        let mut account = records::new_account_record(
            &credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            1,
        )
        .unwrap();
        apply_account_patch(
            &mut account,
            UpdateAccountInput {
                account_id: "account_local".into(),
                label: Some("  Personal  ".into()),
                priority: Some(7),
                weight: Some(2),
                allowed_models: Some(vec![" gpt-test ".into(), "gpt-test".into()]),
                excluded_models: Some(vec![" gpt-old ".into()]),
                in_pool: Some(true),
                draining: Some(true),
            },
        )
        .unwrap();
        assert_eq!(account.account.label, "Personal");
        assert_eq!(account.priority, 7);
        assert_eq!(account.weight, 2);
        assert_eq!(account.allowed_models, ["gpt-test"]);
        assert_eq!(account.excluded_models, ["gpt-old"]);
        assert!(account.account.draining);
        assert!(apply_account_patch(
            &mut account,
            UpdateAccountInput {
                account_id: "account_local".into(),
                label: None,
                priority: None,
                weight: Some(0),
                allowed_models: None,
                excluded_models: None,
                in_pool: None,
                draining: None,
            },
        )
        .is_err());
    }

    #[test]
    fn failed_account_without_models_remains_manageable() {
        let mut account = account_record("account_failed");
        account.models.clear();
        account.account.health = zenith_relay_core::accounts::AccountHealthState::Unhealthy;
        account.account.last_error_code = Some("models_unauthorized".into());
        assert!(account_model_state_is_valid(&account));

        account.account.health = zenith_relay_core::accounts::AccountHealthState::Healthy;
        assert!(!account_model_state_is_valid(&account));
    }

    #[test]
    fn deleting_account_preserves_explicit_empty_key_scope() {
        let mut keys = [LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Scoped".into(),
            enabled: true,
            secret_ref: "key:key_1".into(),
            source_ids: None,
            account_ids: Some(vec!["account_1".into()]),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        }];
        prune_key_account_scopes(&mut keys, &[]);
        assert_eq!(keys[0].account_ids, Some(Vec::new()));
    }

    #[test]
    fn deleting_account_prunes_explicit_selectors_without_rewriting_wake_state() {
        let mut automations = AutomationRecords::default();
        let original_state = automations.state.clone();
        automations.tasks = vec![
            wake_task("only-deleted", &["account_1"]),
            wake_task("shared", &["account_1", "account_2"]),
        ];

        let pruned = prune_account_task_selectors(automations, "account_1");

        assert_eq!(pruned.tasks.len(), 1);
        assert_eq!(pruned.tasks[0].id, "shared");
        assert_eq!(
            pruned.tasks[0].account_selector,
            AccountSelector::AccountIds(BTreeSet::from(["account_2".to_string()]))
        );
        assert_eq!(pruned.state, original_state);
    }

    #[test]
    fn failed_delete_restores_credentials_quota_and_profile_binding() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-delete-rollback-{}",
            Uuid::new_v4().simple()
        ));
        let profile = root.join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("config.toml"), "model_provider = \"custom\"\n").unwrap();
        let state = DesktopState::open(root.clone()).unwrap();
        let account_id = format!("account_{}", Uuid::new_v4().simple());
        let stored = StoredCodexCredentials::new(
            &account_id,
            "access-private".into(),
            Some("refresh-private".into()),
            None,
            Some(60_000),
            1,
            1,
            None,
            Some("provider-private".into()),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        credentials.save(&stored).unwrap();
        state
            .store()
            .unwrap()
            .upsert_account(account_record(&account_id))
            .unwrap();
        state.mark_quota_refresh(&account_id, 123_456).unwrap();
        codex::attach_account(
            &profile,
            &state.profile_backup_root(),
            &account_id,
            &stored.to_token_set().unwrap(),
            "provider-private",
        )
        .unwrap();

        let previous_quota = state.quota_refresh_snapshot().unwrap();
        let previous_wake = state.wake_snapshot().unwrap();
        let old_automations = state.store().unwrap().automations().clone();
        let bindings = codex::account_bindings(&state.profile_backup_root()).unwrap();
        let restored = restore_bound_account_profiles(&state, &bindings, Some(&stored)).unwrap();
        credentials.delete(&account_id).unwrap();
        state.remove_quota_refresh(&account_id).unwrap();

        rollback_deleted_account_side_effects(
            &state,
            &credentials,
            &account_id,
            Some(&stored),
            previous_quota,
            previous_wake,
            old_automations,
            &restored,
            &LocalPoolError::new(ErrorCode::Io, "injected delete failure"),
        )
        .unwrap();

        assert!(credentials.require(&account_id).is_ok());
        assert_eq!(state.next_quota_refresh_due().unwrap(), Some(123_456));
        assert_eq!(
            codex::account_bindings(&state.profile_backup_root())
                .unwrap()
                .len(),
            1
        );
        assert!(std::fs::read_to_string(profile.join("auth.json"))
            .unwrap()
            .contains("access-private"));

        codex::restore_account_profile(&profile, &state.profile_backup_root()).unwrap();
        credentials.delete(&account_id).unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quota_refresh_schedule_uses_reset_lead_and_failure_backoff() {
        let now_ms = 100_000;
        let mut account = account_record("account_local");
        account.account.quota.primary = Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            available_basis_points: Some(10_000),
            explicitly_full: Some(true),
            reset_at_ms: Some(now_ms + 300_000),
            window_minutes: Some(300),
            observed_at_ms: now_ms,
            full_transition_fingerprint: None,
        });
        let updated = AccountQuotaRefreshResponse {
            account: account.clone(),
            quota: AccountQuotaOutcome::Updated {
                transitions: Vec::new(),
            },
        };
        assert_eq!(
            next_quota_refresh_at(&updated, now_ms, 300),
            Some(now_ms + 300_000 - QUOTA_REFRESH_LEAD_MS)
        );
        account.account.quota.primary.as_mut().unwrap().reset_at_ms =
            Some(now_ms + 5 * 60 * 60_000);
        let long_window = AccountQuotaRefreshResponse {
            account: account.clone(),
            quota: AccountQuotaOutcome::Updated {
                transitions: Vec::new(),
            },
        };
        assert_eq!(
            next_quota_refresh_at(&long_window, now_ms, 120),
            Some(now_ms + 120_000)
        );

        let retryable = AccountQuotaRefreshResponse {
            account: account.clone(),
            quota: AccountQuotaOutcome::Failed {
                code: "quota_transport".into(),
                retryable: true,
            },
        };
        assert_eq!(
            next_quota_refresh_at(&retryable, now_ms, 300),
            Some(now_ms + QUOTA_REFRESH_RETRY_MS)
        );
        let terminal = AccountQuotaRefreshResponse {
            account,
            quota: AccountQuotaOutcome::Failed {
                code: "quota_unauthorized".into(),
                retryable: false,
            },
        };
        assert_eq!(next_quota_refresh_at(&terminal, now_ms, 300), None);
        assert_eq!(QUOTA_REFRESH_BATCH_SIZE, 5);
    }

    #[test]
    fn prepared_credentials_debug_output_is_redacted() {
        let prepared = PreparedAccountCredentials {
            tokens: TokenSet::new(
                "access-private",
                Some("refresh-private".into()),
                None,
                Some(60_000),
                1,
                0,
            )
            .unwrap(),
            provider_account_id: "provider-private".into(),
            proxy: None,
        };
        assert_eq!(prepared.tokens().access_token(), "access-private");
        let debug = format!("{prepared:?}");
        assert!(!debug.contains("access-private"));
        assert!(!debug.contains("refresh-private"));
        assert!(!debug.contains("provider-private"));
    }

    #[test]
    fn session_and_item_responses_never_serialize_secret_material() {
        let parsed = parse_import(
            r#"{"account_id":"provider-private","access_token":"access-private","refresh_token":"refresh-private"}"#,
            None,
            &[],
        )
        .unwrap();
        let response = ImportSessionResponse {
            session_id: "session-safe".into(),
            created_at_ms: 1,
            prepared: false,
            preview: parsed.preview,
        };
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("access-private"));
        assert!(!serialized.contains("refresh-private"));
        assert!(!serialized.contains("provider-private"));
        assert!(!serialized.contains("items"));

        let failed = ImportItemResult::failure(
            "import_0123456789abcdef".into(),
            ImportItemError::new("use_source_import", "use the source import flow"),
        );
        let serialized = serde_json::to_string(&failed).unwrap();
        assert!(!serialized.contains("access-private"));
    }

    #[test]
    fn imported_jwt_claims_supply_account_identity_without_serializing_token() {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "email": "private@example.test",
                "exp": 123,
                "https://api.openai.com/auth": {
                    "chatgpt_plan_type": "pro",
                    "chatgpt_subscription_active_until": 1_767_225_600,
                    "chatgpt_user_id": "user-private",
                    "account_id": "account-private"
                }
            })
            .to_string(),
        );
        let token = format!("header.{payload}.signature");
        let identity = imported_identity(Some(&token), Some(&token));
        assert_eq!(
            identity.provider_account_id.as_deref(),
            Some("account-private")
        );
        assert_eq!(identity.provider_user_id.as_deref(), Some("user-private"));
        assert_eq!(identity.plan_type.as_deref(), Some("pro"));
        assert_eq!(
            identity.subscription_active_until_ms,
            Some(1_767_225_600_000)
        );
        assert_eq!(identity.access_expires_at_ms, Some(123_000));
        assert!(!format!("{:x}", Sha256::digest(token.as_bytes())).contains("private"));
    }

    #[tokio::test]
    async fn account_check_recovers_an_id_from_an_access_only_session() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/accounts/check",
            get(|| async {
                Json(serde_json::json!({
                    "account_ordering": ["workspace-private"],
                    "accounts": {
                        "workspace-private": {
                            "account": { "workspace_id": "workspace-private" }
                        }
                    }
                }))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let endpoint = Url::parse(&format!("http://{address}/accounts/check")).unwrap();

        let account_id = lookup_import_account_id(
            endpoint,
            "synthetic-access-only-token",
            None,
            Duration::from_secs(2),
        )
        .await
        .unwrap();

        assert_eq!(account_id, "workspace-private");
        server.abort();
    }

    #[tokio::test]
    async fn imported_explicit_email_wins_over_shared_token_email() {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "email": "shared@example.test",
                "https://api.openai.com/auth": {
                    "chatgpt_user_id": "shared-user",
                    "chatgpt_account_id": "shared-team"
                }
            })
            .to_string(),
        );
        let token = format!("header.{payload}.signature");
        let mut parsed = parse_import(
            &serde_json::json!({
                "email": "member@example.test",
                "access_token": token
            })
            .to_string(),
            None,
            &[],
        )
        .unwrap();
        let material =
            build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 20)
                .await
                .unwrap();
        assert_eq!(material.email.as_deref(), Some("member@example.test"));
    }

    #[test]
    fn quota_response_types_are_safe_and_serializable() {
        let response = AccountQuotaOutcome::Failed {
            code: "quota_transport".into(),
            retryable: true,
        };
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("quota_transport"));
        assert!(!serialized.contains("Bearer"));
        let _ = WireApi::Responses;
    }

    #[test]
    fn missing_import_secret_requires_recovery() {
        let error = import_session_error(ImportSessionError {
            code: ImportSessionErrorCode::SecretMissing,
            message: "import session secret is missing".into(),
            session_id: None,
            import_code: None,
        });
        assert!(serde_json::to_string(&error)
            .unwrap()
            .contains("recovery_required"));
    }
}

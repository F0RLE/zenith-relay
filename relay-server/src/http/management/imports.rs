use super::{
    account_summary, clean_label, default_weight, normalized_values, store_error, valid_weight,
    validate_secret, validation_error, vault_error, ManagementError,
};
use crate::jobs;
use crate::state::{identity_hint, now_ms, AccountCredential, AppState, ServerAccountRecord};
use crate::store::PendingImport;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use url::{Host, Url};
use zenith_relay_core::accounts::{
    combine_import_documents, parse_import, AccountAuthState, AccountHealthState, ImportAuthMode,
    ImportError, ImportErrorCode, ImportFormat, ImportIssueCode, ImportPreviewRow,
    ImportPreviewStatus, ImportQuotaStatus, ImportWarning, ImportWarningCode, ParsedImport,
    ParsedImportItem, MAX_IMPORT_ITEMS,
};
use zenith_relay_core::protocol::{valid_generated_id, AccountSummary};
use zenith_relay_core::providers::chatgpt::parse_subscription_timestamp_ms;
use zenith_relay_core::quota::{Subscription, SubscriptionInput};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/accounts/import/preview", post(preview_account_import))
        .route("/accounts/import/confirm", post(confirm_account_import))
        .route(
            "/accounts/import/batch/preview",
            post(preview_account_batch_import),
        )
        .route(
            "/accounts/import/batch/confirm",
            post(confirm_account_batch_import),
        )
}

const MAX_SYNCHRONOUS_IMPORT_PROBES: usize = 5;

const DEFAULT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportInput {
    label: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    agent_private_key: Option<String>,
    #[serde(default)]
    agent_runtime_id: Option<String>,
    #[serde(default)]
    agent_task_id: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
    chatgpt_account_id: String,
    responses_url: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_weight")]
    weight: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportPreview {
    session_id: String,
    account_id: String,
    duplicate_account_id: Option<String>,
    label: String,
    identity_hint: String,
    models: Vec<String>,
    auth_state: AccountAuthState,
    expires_at_ms: Option<u64>,
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
    allowed_models: Vec<String>,
    excluded_models: Vec<String>,
    priority: i32,
    weight: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    batch_session_id: Option<String>,
}

pub async fn preview_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AccountImportInput>,
) -> Result<(StatusCode, Json<AccountImportPreview>), ManagementError> {
    cleanup_expired_imports(&state)?;
    let preview = prepare_account_import(&state, input, None)?;
    Ok((StatusCode::CREATED, Json(preview)))
}

fn prepare_account_import(
    state: &AppState,
    input: AccountImportInput,
    batch_session_id: Option<&str>,
) -> Result<AccountImportPreview, ManagementError> {
    let has_agent_identity = input.agent_private_key.is_some()
        || input.agent_runtime_id.is_some()
        || input.agent_task_id.is_some();
    if has_agent_identity {
        let private_key = input.agent_private_key.clone().unwrap_or_default();
        let runtime_id = input.agent_runtime_id.clone().unwrap_or_default();
        match input.agent_task_id.clone() {
            Some(task_id) => zenith_relay_core::providers::chatgpt::AgentIdentityCredential::new(
                private_key,
                runtime_id,
                task_id,
            ),
            None => zenith_relay_core::providers::chatgpt::AgentIdentityCredential::unregistered(
                private_key,
                runtime_id,
            ),
        }
        .map_err(|_| {
            ManagementError::validation(
                "agent_identity_invalid",
                "Agent Identity credential is invalid",
            )
        })?;
    } else {
        validate_secret(&input.access_token, "access token")?;
    }
    if let Some(value) = input.refresh_token.as_deref() {
        validate_secret(value, "refresh token")?;
    }
    if let Some(value) = input.id_token.as_deref() {
        validate_secret(value, "ID token")?;
    }
    let label = redact_import_label(
        clean_label(&input.label, "account label")?,
        &[
            Some(input.access_token.as_str()),
            input.refresh_token.as_deref(),
            input.id_token.as_deref(),
            input.agent_private_key.as_deref(),
            Some(input.chatgpt_account_id.as_str()),
        ],
    );
    let plan_type = input
        .plan_type
        .as_deref()
        .filter(|value| {
            !contains_sensitive(
                value,
                &[
                    Some(input.access_token.as_str()),
                    input.refresh_token.as_deref(),
                    input.id_token.as_deref(),
                    input.agent_private_key.as_deref(),
                    Some(input.chatgpt_account_id.as_str()),
                ],
            )
        })
        .map(str::to_string)
        .and_then(safe_plan_type);
    let chatgpt_account_id = clean_identifier(&input.chatgpt_account_id, "account id")?;
    let responses_url = validate_account_responses_url(input.responses_url.as_deref())?;
    let identity_hint = identity_hint(&chatgpt_account_id);
    let duplicate_account = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.identity_hint == identity_hint);
    let duplicate_account_id = duplicate_account.as_ref().map(|record| record.id.clone());
    let account_id = duplicate_account_id
        .clone()
        .unwrap_or_else(|| format!("account_{}", uuid::Uuid::new_v4().simple()));
    let session_id = format!("import_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("account:{account_id}:{}", uuid::Uuid::new_v4().simple());
    let existing_credential = match duplicate_account.as_ref() {
        Some(record) => match state.vault.load(&record.secret_ref).map_err(vault_error)? {
            Some(value) => Some(serde_json::from_str::<AccountCredential>(&value).map_err(
                |_| {
                    ManagementError::internal("account_secret_invalid", "account secret is invalid")
                },
            )?),
            None => None,
        },
        None => None,
    };
    let proxy_url = existing_credential
        .as_ref()
        .and_then(|credential| credential.proxy_url.clone());
    let credential = AccountCredential {
        access_token: input.access_token,
        refresh_token: nonempty(input.refresh_token).or_else(|| {
            existing_credential
                .as_ref()
                .and_then(|credential| credential.refresh_token.clone())
        }),
        id_token: nonempty(input.id_token),
        expires_at_ms: input.expires_at_ms,
        issued_at_ms: now_ms(),
        generation: 0,
        chatgpt_account_id,
        responses_url,
        proxy_url,
        agent_private_key: input.agent_private_key,
        agent_runtime_id: input.agent_runtime_id,
        agent_task_id: input.agent_task_id,
    };
    if credential.is_agent_identity() {
        credential.agent_identity().map_err(validation_error)?;
    }
    if credential.has_oauth() {
        credential.tokens().map_err(validation_error)?;
    }
    if !credential.is_agent_identity() && !credential.has_oauth() {
        return Err(validation_error(
            "account credential has no authorization method",
        ));
    }
    let auth_state = if credential.is_agent_identity() || credential.refresh_token.is_some() {
        AccountAuthState::Active
    } else {
        AccountAuthState::DegradedAccessOnly
    };
    let preview = AccountImportPreview {
        session_id: session_id.clone(),
        account_id,
        duplicate_account_id,
        label,
        identity_hint,
        models: normalized_values(input.models),
        auth_state,
        expires_at_ms: credential.expires_at_ms,
        plan_type,
        subscription_active_until_ms: input.subscription_active_until_ms,
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        batch_session_id: batch_session_id.map(str::to_string),
    };
    state
        .vault
        .save(
            &secret_ref,
            &serde_json::to_string(&credential).map_err(|_| {
                ManagementError::internal("import_serialize", "import could not be prepared")
            })?,
        )
        .map_err(vault_error)?;
    let pending = PendingImport {
        id: session_id,
        preview_json: serde_json::to_string(&preview).map_err(|_| {
            ManagementError::internal("preview_serialize", "preview could not be saved")
        })?,
        secret_ref,
        created_at_ms: now_ms(),
    };
    if let Err(error) = state.store.save_pending_import(&pending) {
        let _ = state.vault.delete(&pending.secret_ref);
        return Err(store_error(error));
    }
    Ok(preview)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportPreviewInput {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    documents: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportSession {
    session_id: String,
    prepared: bool,
    preview: BatchImportPreview,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportPreview {
    format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    rows: Vec<BatchImportRow>,
    warnings: Vec<BatchImportWarning>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportRow {
    item_id: String,
    label: String,
    identity: String,
    auth_mode: String,
    source_name: String,
    quota_status: String,
    status: String,
    plan: Option<String>,
    default_selected: bool,
    selectable: bool,
    existing: bool,
    warnings: Vec<BatchImportWarning>,
    error: Option<BatchImportIssue>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportWarning {
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
}

#[derive(Serialize)]
struct BatchImportIssue {
    code: String,
    message: String,
}

pub async fn preview_account_batch_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<BatchImportPreviewInput>,
) -> Result<(StatusCode, Json<BatchImportSession>), ManagementError> {
    cleanup_expired_imports(&state)?;
    let parsed = parse_batch_import_input(input)?;
    let format = import_format_name(parsed.preview.format).to_string();
    let description = parsed.preview.description.clone();
    let warnings = parsed
        .preview
        .warnings
        .iter()
        .map(batch_import_warning)
        .collect();
    let mut items = parsed
        .items
        .into_iter()
        .map(|item| (item.item_id.clone(), item))
        .collect::<HashMap<_, _>>();
    let session_id = format!("batch_{}", uuid::Uuid::new_v4().simple());
    let mut rows = Vec::with_capacity(parsed.preview.rows.len());
    for preview_row in parsed.preview.rows {
        let row = match items.remove(&preview_row.item_id) {
            Some(item) => match parsed_account_import_input(item, &preview_row)
                .and_then(|input| prepare_account_import(&state, input, Some(&session_id)))
            {
                Ok(preview) => BatchImportRow {
                    item_id: preview.session_id.clone(),
                    label: preview.label,
                    identity: preview.identity_hint,
                    auth_mode: import_auth_mode_name(preview_row.auth_mode).to_string(),
                    source_name: preview_row.source_name.clone(),
                    quota_status: import_quota_status_name(preview_row.quota_status).to_string(),
                    status: if preview.duplicate_account_id.is_some() {
                        "existing".to_string()
                    } else {
                        "ready".to_string()
                    },
                    plan: preview.plan_type,
                    default_selected: preview.duplicate_account_id.is_none(),
                    selectable: true,
                    existing: preview.duplicate_account_id.is_some(),
                    warnings: preview_row
                        .warnings
                        .iter()
                        .map(batch_import_warning)
                        .collect(),
                    error: None,
                },
                Err(error) => invalid_shared_batch_row(
                    preview_row.item_id,
                    preview_row.label,
                    preview_row.identity,
                    preview_row.source_name,
                    error.code,
                    error.message,
                ),
            },
            None => batch_preview_row(preview_row),
        };
        rows.push(row);
    }
    Ok((
        StatusCode::CREATED,
        Json(BatchImportSession {
            session_id,
            prepared: true,
            preview: BatchImportPreview {
                format,
                description,
                rows,
                warnings,
            },
        }),
    ))
}

fn parse_batch_import_input(
    input: BatchImportPreviewInput,
) -> Result<ParsedImport, ManagementError> {
    let content = input.content.filter(|value| !value.trim().is_empty());
    let content = if input.documents.is_empty() {
        content.unwrap_or_default()
    } else if content.is_some() {
        return Err(ManagementError::validation(
            "import_input_conflict",
            "paste content and file documents cannot be imported together",
        ));
    } else if input.documents.len() == 1 {
        input.documents.into_iter().next().unwrap_or_default()
    } else {
        combine_import_documents(&input.documents).map_err(import_error)?
    };
    parse_import(&content, None, &[]).map_err(import_error)
}

fn parsed_account_import_input(
    item: ParsedImportItem,
    preview: &ImportPreviewRow,
) -> Result<AccountImportInput, ManagementError> {
    if preview.auth_mode == ImportAuthMode::ApiKey {
        return Err(ManagementError::validation(
            "unsupported_value",
            "API keys must be imported as API sources, not pool accounts",
        ));
    }
    let account_id = item.account_id.clone().ok_or_else(|| {
        ManagementError::validation(
            "missing_account_id",
            "account import requires an account id",
        )
    })?;
    let secrets = item.secrets();
    Ok(AccountImportInput {
        label: item.label.clone(),
        access_token: secrets.access_token().unwrap_or_default().to_string(),
        agent_private_key: secrets.agent_private_key().map(str::to_string),
        agent_runtime_id: secrets.agent_runtime_id().map(str::to_string),
        agent_task_id: secrets.agent_task_id().map(str::to_string),
        refresh_token: secrets.refresh_token().map(str::to_string),
        id_token: secrets.id_token().map(str::to_string),
        expires_at_ms: preview
            .expires_at
            .as_ref()
            .and_then(|value| parse_subscription_timestamp_ms(&Value::String(value.clone()))),
        plan_type: preview.plan.clone(),
        subscription_active_until_ms: preview
            .subscription_expires_at
            .as_ref()
            .and_then(|value| parse_subscription_timestamp_ms(&Value::String(value.clone()))),
        chatgpt_account_id: account_id,
        responses_url: item.base_url.clone(),
        models: Vec::new(),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: item.priority.unwrap_or_default(),
        weight: default_weight(),
    })
}

fn batch_preview_row(row: ImportPreviewRow) -> BatchImportRow {
    BatchImportRow {
        item_id: row.item_id,
        label: row.label,
        identity: row.identity,
        auth_mode: import_auth_mode_name(row.auth_mode).to_string(),
        source_name: row.source_name,
        quota_status: import_quota_status_name(row.quota_status).to_string(),
        status: import_preview_status_name(row.status).to_string(),
        plan: row.plan,
        default_selected: row.default_selected,
        selectable: row.selectable,
        existing: row.existing,
        warnings: row.warnings.iter().map(batch_import_warning).collect(),
        error: row.error.map(|error| BatchImportIssue {
            code: import_issue_code_name(error.code).to_string(),
            message: error.message,
        }),
    }
}

fn invalid_shared_batch_row(
    item_id: String,
    label: String,
    identity: String,
    source_name: String,
    code: String,
    message: String,
) -> BatchImportRow {
    BatchImportRow {
        item_id,
        label,
        identity,
        auth_mode: "unknown".to_string(),
        source_name,
        quota_status: "skipped".to_string(),
        status: "invalid".to_string(),
        plan: None,
        default_selected: false,
        selectable: false,
        existing: false,
        warnings: Vec::new(),
        error: Some(BatchImportIssue { code, message }),
    }
}

fn batch_import_warning(warning: &ImportWarning) -> BatchImportWarning {
    BatchImportWarning {
        code: import_warning_code_name(warning.code).to_string(),
        count: warning.count,
    }
}

fn import_error(error: ImportError) -> ManagementError {
    let code = match error.code {
        ImportErrorCode::EmptyInput => "import_empty",
        ImportErrorCode::InputTooLarge => "import_too_large",
        ImportErrorCode::InvalidSourceFile => "import_source_file_invalid",
        ImportErrorCode::JsonTooDeep => "import_too_deep",
        ImportErrorCode::MalformedJson => "import_malformed",
        ImportErrorCode::TooManyItems => "import_item_count",
        ImportErrorCode::UnsupportedBundleVersion => "unsupported_bundle_version",
    };
    ManagementError::validation(code, error.message)
}

fn import_format_name(value: ImportFormat) -> &'static str {
    match value {
        ImportFormat::JsonObject => "json_object",
        ImportFormat::JsonArray => "json_array",
        ImportFormat::JsonLines => "json_lines",
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
        ImportFormat::ZenithV1 => "zenith_v1",
    }
}

fn import_auth_mode_name(value: ImportAuthMode) -> &'static str {
    match value {
        ImportAuthMode::OAuth => "oauth",
        ImportAuthMode::AgentIdentity => "agent_identity",
        ImportAuthMode::ApiKey => "api_key",
        ImportAuthMode::ImportedToken => "imported_token",
        ImportAuthMode::Unknown => "unknown",
    }
}

fn import_preview_status_name(value: ImportPreviewStatus) -> &'static str {
    match value {
        ImportPreviewStatus::Ready => "ready",
        ImportPreviewStatus::Existing => "existing",
        ImportPreviewStatus::QuotaFailed => "quota_failed",
        ImportPreviewStatus::Invalid => "invalid",
    }
}

fn import_quota_status_name(value: ImportQuotaStatus) -> &'static str {
    match value {
        ImportQuotaStatus::Skipped => "skipped",
        ImportQuotaStatus::Success => "success",
        ImportQuotaStatus::Failed => "failed",
    }
}

fn import_warning_code_name(value: ImportWarningCode) -> &'static str {
    match value {
        ImportWarningCode::AccessTokenOnly => "access_token_only",
        ImportWarningCode::ConcurrencyIgnored => "concurrency_ignored",
        ImportWarningCode::InvalidMetadataIgnored => "invalid_metadata_ignored",
        ImportWarningCode::ProxiesIgnored => "proxies_ignored",
        ImportWarningCode::RefreshExchangeRequired => "refresh_exchange_required",
        ImportWarningCode::UnusedCredentialsIgnored => "unused_credentials_ignored",
        ImportWarningCode::UnknownAuthMode => "unknown_auth_mode",
    }
}

fn import_issue_code_name(value: ImportIssueCode) -> &'static str {
    match value {
        ImportIssueCode::AmbiguousCredentials => "ambiguous_credentials",
        ImportIssueCode::DuplicateItem => "duplicate_item",
        ImportIssueCode::InvalidCredentials => "invalid_credentials",
        ImportIssueCode::MalformedJson => "malformed_json",
        ImportIssueCode::MissingCredentials => "missing_credentials",
        ImportIssueCode::QuotaProbeFailed => "quota_probe_failed",
        ImportIssueCode::RefreshExchangeFailed => "refresh_exchange_failed",
        ImportIssueCode::UnsupportedValue => "unsupported_value",
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportConfirmInput {
    session_id: String,
    selected_item_ids: Vec<String>,
    #[serde(default)]
    add_to_pool: bool,
    #[serde(default)]
    probe_metadata: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportConfirmResponse {
    session_id: String,
    results: Vec<BatchImportResult>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportResult {
    item_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<BatchImportIssue>,
}

pub async fn confirm_account_batch_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<BatchImportConfirmInput>,
) -> Result<Json<BatchImportConfirmResponse>, ManagementError> {
    if !valid_generated_id(&input.session_id, "batch_") {
        return Err(ManagementError::validation(
            "import_session_invalid",
            "batch import session is invalid",
        ));
    }
    if input.selected_item_ids.is_empty() || input.selected_item_ids.len() > MAX_IMPORT_ITEMS {
        return Err(ManagementError::validation(
            "import_selection_invalid",
            format!("import selection must contain between 1 and {MAX_IMPORT_ITEMS} items"),
        ));
    }
    let probe_metadata =
        input.probe_metadata && input.selected_item_ids.len() <= MAX_SYNCHRONOUS_IMPORT_PROBES;
    let mut seen = HashSet::new();
    let mut results = Vec::with_capacity(input.selected_item_ids.len());
    for item_id in input.selected_item_ids {
        if !seen.insert(item_id.clone()) {
            continue;
        }
        let result = match confirm_one_account_import(
            &state,
            &item_id,
            Some(&input.session_id),
            input.add_to_pool,
            probe_metadata,
        )
        .await
        {
            Ok(account) => BatchImportResult {
                item_id,
                status: "succeeded".to_string(),
                account_id: Some(account.id),
                error: None,
            },
            Err(error) => BatchImportResult {
                item_id,
                status: "failed".to_string(),
                account_id: None,
                error: Some(BatchImportIssue {
                    code: error.code,
                    message: error.message,
                }),
            },
        };
        results.push(result);
    }
    Ok(Json(BatchImportConfirmResponse {
        session_id: input.session_id,
        results,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmImportInput {
    session_id: String,
    #[serde(default)]
    add_to_pool: bool,
    #[serde(default)]
    probe_metadata: bool,
}

pub async fn confirm_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ConfirmImportInput>,
) -> Result<Json<AccountSummary>, ManagementError> {
    confirm_one_account_import(
        &state,
        &input.session_id,
        None,
        input.add_to_pool,
        input.probe_metadata,
    )
    .await
    .map(Json)
}

async fn confirm_one_account_import(
    state: &Arc<AppState>,
    session_id: &str,
    batch_session_id: Option<&str>,
    add_to_pool: bool,
    probe_metadata: bool,
) -> Result<AccountSummary, ManagementError> {
    if !valid_generated_id(session_id, "import_") {
        return Err(ManagementError::validation(
            "import_session_invalid",
            "account import session is invalid",
        ));
    }
    let pending = state
        .store
        .pending_import(session_id)
        .map_err(store_error)?
        .ok_or_else(|| {
            ManagementError::not_found("import_not_found", "import session not found")
        })?;
    if now_ms().saturating_sub(pending.created_at_ms) > 30 * 60 * 1_000 {
        let _ = state.store.delete_pending_import(session_id);
        let _ = state.vault.delete(&pending.secret_ref);
        return Err(ManagementError::validation(
            "import_expired",
            "import session expired",
        ));
    }
    let preview: AccountImportPreview =
        serde_json::from_str(&pending.preview_json).map_err(|_| {
            ManagementError::internal("preview_invalid", "stored import preview is invalid")
        })?;
    if preview.batch_session_id.as_deref() != batch_session_id {
        return Err(ManagementError::not_found(
            "import_not_found",
            "import session not found",
        ));
    }
    let existing = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == preview.account_id);
    let mut subscription =
        if preview.plan_type.is_some() || preview.subscription_active_until_ms.is_some() {
            Subscription::normalize(SubscriptionInput {
                plan_type: preview.plan_type.clone(),
                active_until_ms: preview.subscription_active_until_ms,
                forbidden: false,
                observed_at_ms: now_ms(),
            })
        } else {
            existing
                .as_ref()
                .map(|value| value.subscription.clone())
                .unwrap_or_default()
        };
    if subscription.active_until_ms.is_none() {
        subscription.updated_at_ms = None;
    }
    let mut record = ServerAccountRecord {
        id: preview.account_id.clone(),
        label: existing
            .as_ref()
            .map(|value| value.label.clone())
            .unwrap_or(preview.label),
        identity_hint: preview.identity_hint,
        enabled: existing.as_ref().is_none_or(|value| value.enabled),
        in_pool: add_to_pool || existing.as_ref().is_some_and(|value| value.in_pool),
        draining: existing.as_ref().is_some_and(|value| value.draining),
        source_id: "openai_codex".to_string(),
        secret_ref: pending.secret_ref.clone(),
        auth_state: preview.auth_state,
        health: AccountHealthState::Healthy,
        models: existing
            .as_ref()
            .map(|value| value.models.clone())
            .unwrap_or(preview.models),
        allowed_models: existing
            .as_ref()
            .map(|value| value.allowed_models.clone())
            .unwrap_or(preview.allowed_models),
        excluded_models: existing
            .as_ref()
            .map(|value| value.excluded_models.clone())
            .unwrap_or(preview.excluded_models),
        priority: existing
            .as_ref()
            .map_or(preview.priority, |value| value.priority),
        weight: existing
            .as_ref()
            .map_or(preview.weight, |value| value.weight),
        subscription,
        quota: existing
            .as_ref()
            .map(|value| value.quota.clone())
            .unwrap_or_default(),
        economics: existing
            .as_ref()
            .map(|value| value.economics.clone())
            .unwrap_or_default(),
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        created_at_ms: existing
            .as_ref()
            .map(|value| value.created_at_ms)
            .filter(|value| *value > 0)
            .unwrap_or(pending.created_at_ms),
        last_used_at_ms: existing.as_ref().and_then(|value| value.last_used_at_ms),
        last_error_code: None,
        proxy_id: existing.as_ref().and_then(|value| value.proxy_id.clone()),
        bypass_common_proxy: existing
            .as_ref()
            .is_some_and(|value| value.bypass_common_proxy),
    };
    state.store.save_account(&record).map_err(store_error)?;
    if probe_metadata {
        match jobs::refresh_account_now(state, record.clone()).await {
            Ok(updated) => record = updated,
            Err(_) => {
                record.health = AccountHealthState::Degraded;
                record.last_error_code = Some("metadata_refresh_failed".to_string());
                let _ = state.store.save_account(&record);
            }
        }
    } else if state.rebuild_runtime().await.is_err() {
        record.health = AccountHealthState::Degraded;
        record.last_error_code = Some("runtime_rebuild_failed".to_string());
        let _ = state.store.save_account(&record);
    }
    state
        .store
        .delete_pending_import(session_id)
        .map_err(store_error)?;
    if let Some(previous) = existing {
        if previous.secret_ref != record.secret_ref {
            let _ = state.vault.delete(&previous.secret_ref);
        }
    }
    account_summary(state, &record)
}

fn cleanup_expired_imports(state: &AppState) -> Result<(), ManagementError> {
    let cutoff = now_ms().saturating_sub(30 * 60 * 1_000);
    for secret_ref in state
        .store
        .delete_pending_imports_before(cutoff)
        .map_err(store_error)?
    {
        let _ = state.vault.delete(&secret_ref);
    }
    Ok(())
}

fn safe_plan_type(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_lowercase())
}

fn redact_import_label(value: String, sensitive: &[Option<&str>]) -> String {
    if contains_sensitive(&value, sensitive) {
        "Imported account".to_string()
    } else {
        value
    }
}

fn contains_sensitive(value: &str, sensitive: &[Option<&str>]) -> bool {
    sensitive
        .iter()
        .flatten()
        .any(|secret| secret.len() >= 4 && value.contains(secret))
}

fn validate_account_responses_url(value: Option<&str>) -> Result<String, ManagementError> {
    let value = value.unwrap_or(DEFAULT_CODEX_RESPONSES_URL).trim();
    let url =
        Url::parse(value).map_err(|_| validation_error("account responses URL is invalid"))?;
    let loopback = match url.host() {
        Some(Host::Ipv4(value)) => value.is_loopback(),
        Some(Host::Ipv6(value)) => value.is_loopback(),
        Some(Host::Domain(value)) => value.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    let allowed = (url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("chatgpt.com")))
        || (url.scheme() == "http" && loopback);
    if !allowed
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(validation_error("account responses URL is not allowed"));
    }
    Ok(url.to_string())
}

fn clean_identifier(value: &str, name: &str) -> Result<String, ManagementError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(validation_error(format!("{name} is invalid")))
    } else {
        Ok(value.to_string())
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{parse_batch_import_input, BatchImportPreviewInput};

    #[test]
    fn batch_import_parses_raw_token_lines() {
        let parsed = parse_batch_import_input(BatchImportPreviewInput {
            content: Some("Bearer header.payload.signature\nat-opaque-token".into()),
            documents: Vec::new(),
        })
        .unwrap();

        assert_eq!(parsed.items.len(), 2);
        assert_eq!(
            parsed.items[0].secrets().access_token(),
            Some("header.payload.signature")
        );
        assert_eq!(
            parsed.items[1].secrets().access_token(),
            Some("at-opaque-token")
        );
    }

    #[test]
    fn batch_import_keeps_valid_documents_when_one_is_malformed() {
        let parsed = parse_batch_import_input(BatchImportPreviewInput {
            content: None,
            documents: vec![
                r#"{"account_id":"account-one","access_token":"access-one"}"#.into(),
                r#"{"access_token":"truncated""#.into(),
            ],
        })
        .unwrap();

        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.preview.rows.len(), 2);
        assert!(parsed.preview.rows[0].error.is_none());
        assert_eq!(
            parsed.preview.rows[1].error.as_ref().unwrap().code,
            zenith_relay_core::accounts::ImportIssueCode::MalformedJson
        );
    }

    #[test]
    fn batch_import_accepts_sub2api_agent_identity() {
        let input = serde_json::json!({
            "name": "Agent account",
            "credentials": {
                "auth_mode": "agentIdentity",
                "agent_private_key": "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g",
                "agent_runtime_id": "runtime-test",
                "task_id": "task-test",
                "chatgpt_account_id": "account-test"
            }
        });
        let parsed = parse_batch_import_input(BatchImportPreviewInput {
            content: Some(input.to_string()),
            documents: Vec::new(),
        })
        .unwrap();
        let normalized = &parsed.items[0];

        assert!(normalized.secrets().access_token().is_none());
        assert_eq!(
            normalized.secrets().agent_runtime_id(),
            Some("runtime-test")
        );
        assert_eq!(normalized.secrets().agent_task_id(), Some("task-test"));
        assert!(
            zenith_relay_core::providers::chatgpt::AgentIdentityCredential::new(
                normalized
                    .secrets()
                    .agent_private_key()
                    .unwrap()
                    .to_string(),
                normalized.secrets().agent_runtime_id().unwrap().to_string(),
                normalized.secrets().agent_task_id().unwrap().to_string(),
            )
            .is_ok()
        );
    }
}

use super::super::{default_weight, ManagementError};
use super::confirm::ConfirmedAccountImport;
use super::confirm::{cleanup_expired_imports, confirm_one_account_import};
use super::preview::{prepare_account_import, AccountImportInput};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use zenith_relay_core::accounts::{
    combine_import_documents, parse_import, ImportAuthMode, ImportError, ImportErrorCode,
    ImportFormat, ImportIssueCode, ImportPreviewRow, ImportPreviewStatus, ImportQuotaStatus,
    ImportWarning, ImportWarningCode, ParsedImport, ParsedImportItem, MAX_IMPORT_ITEMS,
};
use zenith_relay_core::error_codes;

use zenith_relay_core::protocol::valid_generated_id;
use zenith_relay_core::providers::chatgpt::{parse_subscription_timestamp_ms, OAuthClientKind};

const MAX_SYNCHRONOUS_IMPORT_PROBES: usize = 5;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportPreviewInput {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) documents: Vec<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    oauth_client_kind: Option<OAuthClientKind>,
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
    let mut parsed_items_by_id = parsed
        .items
        .into_iter()
        .map(|import_item| (import_item.item_id.clone(), import_item))
        .collect::<HashMap<_, _>>();
    let session_id = format!("batch_{}", uuid::Uuid::new_v4().simple());
    let mut rows = Vec::with_capacity(parsed.preview.rows.len());
    for preview_row in parsed.preview.rows {
        let row = match parsed_items_by_id.remove(&preview_row.item_id) {
            Some(import_item) => {
                let prepared = match parsed_account_import_input(import_item, &preview_row) {
                    Ok(input) => prepare_account_import(&state, input, Some(&session_id)).await,
                    Err(error) => Err(error),
                };
                match prepared {
                    Ok(preview) => BatchImportRow {
                        item_id: preview.session_id.clone(),
                        label: preview.label,
                        identity: preview.identity_hint,
                        auth_mode: preview_row.auth_mode.as_str().to_string(),
                        oauth_client_kind: Some(preview.oauth_client_kind),
                        source_name: preview_row.source_name.clone(),
                        quota_status: import_quota_status_name(preview_row.quota_status)
                            .to_string(),
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
                }
            }
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

pub(super) fn parse_batch_import_input(
    input: BatchImportPreviewInput,
) -> Result<ParsedImport, ManagementError> {
    let pasted_import_text = zenith_relay_core::omit_blank(input.content);
    let combined_import_text = if input.documents.is_empty() {
        pasted_import_text.unwrap_or_default()
    } else if pasted_import_text.is_some() {
        return Err(ManagementError::validation(
            error_codes::IMPORT_INPUT_CONFLICT,
            "paste content and file documents cannot be imported together",
        ));
    } else if input.documents.len() == 1 {
        input.documents.into_iter().next().unwrap_or_default()
    } else {
        combine_import_documents(&input.documents).map_err(import_error)?
    };
    parse_import(&combined_import_text, None, &[]).map_err(import_error)
}

fn parsed_account_import_input(
    import_item: ParsedImportItem,
    preview: &ImportPreviewRow,
) -> Result<AccountImportInput, ManagementError> {
    if preview.auth_mode == ImportAuthMode::ApiKey {
        return Err(ManagementError::validation(
            error_codes::UNSUPPORTED_VALUE,
            "API keys must be imported as API sources, not pool accounts",
        ));
    }
    let account_id = import_item.account_id.clone();
    let secrets = import_item.secrets();
    Ok(AccountImportInput {
        label: import_item.label.clone(),
        oauth_client_kind: secrets.oauth_client_kind().unwrap_or_default(),
        chatgpt_user_id: import_item.chatgpt_user_id.clone(),
        basis_points_headers: secrets.basis_points_headers().cloned(),
        access_token: secrets.access_token().unwrap_or_default().to_string(),
        agent_private_key: secrets.agent_private_key().map(str::to_string),
        agent_runtime_id: secrets.agent_runtime_id().map(str::to_string),
        agent_task_id: secrets.agent_task_id().map(str::to_string),
        refresh_token: secrets.refresh_token().map(str::to_string),
        id_token: secrets.id_token().map(str::to_string),
        expires_at_ms: preview.expires_at.as_ref().and_then(|timestamp_text| {
            parse_subscription_timestamp_ms(&Value::String(timestamp_text.clone()))
        }),
        plan_type: preview.plan.clone(),
        subscription_active_until_ms: preview.subscription_expires_at.as_ref().and_then(
            |timestamp_text| {
                parse_subscription_timestamp_ms(&Value::String(timestamp_text.clone()))
            },
        ),
        chatgpt_account_id: account_id,
        responses_url: import_item.base_url.clone(),
        models: Vec::new(),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: import_item.priority.unwrap_or_default(),
        weight: default_weight(),
    })
}

fn batch_preview_row(row: ImportPreviewRow) -> BatchImportRow {
    BatchImportRow {
        item_id: row.item_id,
        label: row.label,
        identity: row.identity,
        auth_mode: row.auth_mode.as_str().to_string(),
        oauth_client_kind: row.oauth_client_kind,
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
        oauth_client_kind: None,
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
        ImportErrorCode::UnsupportedBundleVersion => error_codes::UNSUPPORTED_BUNDLE_VERSION,
    };
    ManagementError::validation(code, error.message)
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
    created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<BatchImportIssue>,
}

pub async fn confirm_account_batch_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<BatchImportConfirmInput>,
) -> Result<Json<BatchImportConfirmResponse>, ManagementError> {
    if !valid_generated_id(&input.session_id, "batch_") {
        return Err(ManagementError::validation(
            error_codes::IMPORT_SESSION_INVALID,
            "batch import session is invalid",
        ));
    }
    if input.selected_item_ids.is_empty() || input.selected_item_ids.len() > MAX_IMPORT_ITEMS {
        return Err(ManagementError::validation(
            error_codes::IMPORT_SELECTION_INVALID,
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
        let import_result = match confirm_one_account_import(
            &state,
            &item_id,
            Some(&input.session_id),
            input.add_to_pool,
            probe_metadata,
        )
        .await
        {
            Ok(ConfirmedAccountImport { account, created }) => BatchImportResult {
                item_id,
                status: "succeeded".to_string(),
                account_id: Some(account.id),
                created,
                error: None,
            },
            Err(error) => BatchImportResult {
                item_id,
                status: "failed".to_string(),
                account_id: None,
                created: false,
                error: Some(BatchImportIssue {
                    code: error.code,
                    message: error.message,
                }),
            },
        };
        results.push(import_result);
    }
    Ok(Json(BatchImportConfirmResponse {
        session_id: input.session_id,
        results,
    }))
}

fn import_format_name(import_format: ImportFormat) -> &'static str {
    match import_format {
        ImportFormat::JsonObject => "json_object",
        ImportFormat::JsonArray => "json_array",
        ImportFormat::JsonLines => "json_lines",
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
        ImportFormat::ZenithV1 => "zenith_v1",
    }
}

fn import_preview_status_name(preview_status: ImportPreviewStatus) -> &'static str {
    match preview_status {
        ImportPreviewStatus::Ready => "ready",
        ImportPreviewStatus::Existing => "existing",
        ImportPreviewStatus::QuotaFailed => "quota_failed",
        ImportPreviewStatus::Invalid => "invalid",
    }
}

fn import_quota_status_name(quota_status: ImportQuotaStatus) -> &'static str {
    match quota_status {
        ImportQuotaStatus::Skipped => "skipped",
        ImportQuotaStatus::Success => "success",
        ImportQuotaStatus::Failed => "failed",
    }
}

fn import_warning_code_name(warning_code: ImportWarningCode) -> &'static str {
    match warning_code {
        ImportWarningCode::AccessTokenOnly => "access_token_only",
        ImportWarningCode::ConcurrencyIgnored => "concurrency_ignored",
        ImportWarningCode::InvalidMetadataIgnored => "invalid_metadata_ignored",
        ImportWarningCode::ProxiesIgnored => "proxies_ignored",
        ImportWarningCode::RefreshExchangeRequired => "refresh_exchange_required",
        ImportWarningCode::UnusedCredentialsIgnored => "unused_credentials_ignored",
        ImportWarningCode::UnknownAuthMode => error_codes::UNKNOWN_AUTH_MODE,
    }
}

fn import_issue_code_name(issue_code: ImportIssueCode) -> &'static str {
    match issue_code {
        ImportIssueCode::AmbiguousCredentials => error_codes::AMBIGUOUS_CREDENTIALS,
        ImportIssueCode::DuplicateItem => error_codes::DUPLICATE_ITEM,
        ImportIssueCode::InvalidCredentials => error_codes::INVALID_CREDENTIALS,
        ImportIssueCode::MalformedJson => error_codes::MALFORMED_JSON,
        ImportIssueCode::MissingCredentials => error_codes::MISSING_CREDENTIALS,
        ImportIssueCode::QuotaProbeFailed => error_codes::QUOTA_PROBE_FAILED,
        ImportIssueCode::RefreshExchangeFailed => error_codes::REFRESH_EXCHANGE_FAILED,
        ImportIssueCode::UnsupportedValue => error_codes::UNSUPPORTED_VALUE,
    }
}

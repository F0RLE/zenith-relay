use super::normalize_account_export_description;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::HashSet, fmt};

mod formats;
mod item;
mod sanitization;

use formats::{malformed_import_value, parse_entries};
use item::parse_item;
use sanitization::{redact_file_name, sha256_hex};

pub const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMPORT_ITEMS: usize = 1_024;
pub const MAX_JSON_DEPTH: usize = 32;
const MAX_RAW_TOKEN_BYTES: usize = 64 * 1024;
const IMPORT_ERROR_MARKER: &str = "__zenith_import_error";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    JsonObject,
    JsonArray,
    JsonLines,
    PortableAccountBundleV1,
    ZenithV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportAuthMode {
    OAuth,
    AgentIdentity,
    ApiKey,
    ImportedToken,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportPreviewStatus {
    Ready,
    Existing,
    QuotaFailed,
    Invalid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportQuotaStatus {
    Skipped,
    Success,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportWarningCode {
    AccessTokenOnly,
    ConcurrencyIgnored,
    InvalidMetadataIgnored,
    ProxiesIgnored,
    RefreshExchangeRequired,
    UnusedCredentialsIgnored,
    UnknownAuthMode,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportWarning {
    pub code: ImportWarningCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

impl ImportWarning {
    fn new(code: ImportWarningCode) -> Self {
        Self { code, count: None }
    }

    fn count(code: ImportWarningCode, count: usize) -> Self {
        Self {
            code,
            count: Some(count),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportIssueCode {
    AmbiguousCredentials,
    DuplicateItem,
    InvalidCredentials,
    MalformedJson,
    MissingCredentials,
    QuotaProbeFailed,
    RefreshExchangeFailed,
    UnsupportedValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportIssue {
    pub code: ImportIssueCode,
    pub message: String,
}

impl ImportIssue {
    fn new(code: ImportIssueCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportErrorCode {
    EmptyInput,
    InputTooLarge,
    InvalidSourceFile,
    JsonTooDeep,
    MalformedJson,
    TooManyItems,
    UnsupportedBundleVersion,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportError {
    pub code: ImportErrorCode,
    pub message: String,
}

impl ImportError {
    fn new(code: ImportErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreviewRow {
    pub item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    pub label: String,
    pub identity: String,
    pub auth_mode: ImportAuthMode,
    pub source_name: String,
    pub quota_status: ImportQuotaStatus,
    pub status: ImportPreviewStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ImportIssue>,
    pub default_selected: bool,
    pub selectable: bool,
    pub existing: bool,
    pub warnings: Vec<ImportWarning>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub format: ImportFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub rows: Vec<ImportPreviewRow>,
    pub warnings: Vec<ImportWarning>,
}

pub struct RedactedValue(String);

impl RedactedValue {
    fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RedactedValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[derive(Default)]
pub struct ImportSecretMaterial {
    access_token: Option<RedactedValue>,
    refresh_token: Option<RedactedValue>,
    id_token: Option<RedactedValue>,
    api_key: Option<RedactedValue>,
    agent_private_key: Option<RedactedValue>,
    agent_runtime_id: Option<RedactedValue>,
    agent_task_id: Option<RedactedValue>,
}

impl ImportSecretMaterial {
    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_ref().map(RedactedValue::expose)
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_ref().map(RedactedValue::expose)
    }

    pub fn id_token(&self) -> Option<&str> {
        self.id_token.as_ref().map(RedactedValue::expose)
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_ref().map(RedactedValue::expose)
    }

    pub fn agent_private_key(&self) -> Option<&str> {
        self.agent_private_key.as_ref().map(RedactedValue::expose)
    }

    pub fn agent_runtime_id(&self) -> Option<&str> {
        self.agent_runtime_id.as_ref().map(RedactedValue::expose)
    }

    pub fn agent_task_id(&self) -> Option<&str> {
        self.agent_task_id.as_ref().map(RedactedValue::expose)
    }
}

impl fmt::Debug for ImportSecretMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportSecretMaterial")
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field(
                "agent_private_key",
                &self.agent_private_key.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "agent_runtime_id",
                &self.agent_runtime_id.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "agent_task_id",
                &self.agent_task_id.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

pub struct ParsedImportItem {
    pub item_id: String,
    pub identity_key: String,
    pub label: String,
    pub account_id: Option<String>,
    pub chatgpt_user_id: Option<String>,
    pub organization_id: Option<String>,
    pub base_url: Option<String>,
    pub base_url_supplied: bool,
    pub protocol: Option<String>,
    pub protocol_supplied: bool,
    pub priority: Option<i32>,
    email: Option<RedactedValue>,
    secrets: ImportSecretMaterial,
}

impl ParsedImportItem {
    pub fn email(&self) -> Option<&str> {
        self.email.as_ref().map(RedactedValue::expose)
    }

    pub fn secrets(&self) -> &ImportSecretMaterial {
        &self.secrets
    }

    pub fn into_secrets(self) -> ImportSecretMaterial {
        self.secrets
    }
}

impl fmt::Debug for ParsedImportItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedImportItem")
            .field("item_id", &self.item_id)
            .field("identity_key", &self.identity_key)
            .field("label", &self.label)
            .field("account_id", &self.account_id)
            .field("chatgpt_user_id", &self.chatgpt_user_id)
            .field("organization_id", &self.organization_id)
            .field("base_url", &self.base_url)
            .field("base_url_supplied", &self.base_url_supplied)
            .field("protocol", &self.protocol)
            .field("protocol_supplied", &self.protocol_supplied)
            .field("priority", &self.priority)
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("secrets", &self.secrets)
            .finish()
    }
}

pub struct ParsedImport {
    pub preview: ImportPreview,
    pub items: Vec<ParsedImportItem>,
}

impl fmt::Debug for ParsedImport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedImport")
            .field("preview", &self.preview)
            .field("item_count", &self.items.len())
            .finish()
    }
}

pub fn parse_import(
    input: &str,
    source_file: Option<&str>,
    existing_identity_keys: &[String],
) -> Result<ParsedImport, ImportError> {
    if input.is_empty() || input.trim().is_empty() {
        return Err(ImportError::new(
            ImportErrorCode::EmptyInput,
            "import content is empty",
        ));
    }
    if input.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::new(
            ImportErrorCode::InputTooLarge,
            "import content exceeds the size limit",
        ));
    }
    let source_file = validate_source_file(source_file)?;
    let (format, entries, warnings, description) = parse_entries(input)?;
    if entries.len() > MAX_IMPORT_ITEMS {
        return Err(ImportError::new(
            ImportErrorCode::TooManyItems,
            "import content exceeds the item limit",
        ));
    }

    let existing = existing_identity_keys
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut rows = Vec::with_capacity(entries.len());
    let mut items = Vec::with_capacity(entries.len());

    for entry in entries {
        match entry.value {
            Some(value) => {
                match parse_item(&value, entry.ordinal, format, source_file.as_deref()) {
                    Ok(mut parsed) => {
                        let identity_key = parsed.item.identity_key.to_ascii_lowercase();
                        if !seen.insert(identity_key.clone()) {
                            parsed.preview.status = ImportPreviewStatus::Invalid;
                            parsed.preview.error = Some(ImportIssue::new(
                                ImportIssueCode::DuplicateItem,
                                "duplicate import item",
                            ));
                            parsed.preview.default_selected = false;
                            parsed.preview.selectable = false;
                            rows.push(parsed.preview);
                            continue;
                        }
                        if existing.contains(&identity_key) {
                            parsed.preview.status = ImportPreviewStatus::Existing;
                            parsed.preview.default_selected = false;
                            parsed.preview.existing = true;
                        }
                        rows.push(parsed.preview);
                        items.push(parsed.item);
                    }
                    Err(issue) => rows.push(invalid_row(
                        entry.ordinal,
                        format,
                        source_file.as_deref(),
                        issue,
                    )),
                }
            }
            None => rows.push(invalid_row(
                entry.ordinal,
                format,
                source_file.as_deref(),
                entry.issue.unwrap_or_else(|| {
                    ImportIssue::new(ImportIssueCode::MalformedJson, "malformed JSON item")
                }),
            )),
        }
    }

    Ok(ParsedImport {
        preview: ImportPreview {
            format,
            description,
            rows,
            warnings,
        },
        items,
    })
}

pub fn combine_import_documents(documents: &[String]) -> Result<String, ImportError> {
    if documents.is_empty() {
        return Err(ImportError::new(
            ImportErrorCode::EmptyInput,
            "import content is empty",
        ));
    }

    let mut total_bytes = 0usize;
    let mut values = Vec::new();
    for document in documents {
        total_bytes = total_bytes.checked_add(document.len()).ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::InputTooLarge,
                "import content exceeds the size limit",
            )
        })?;
        if total_bytes > MAX_IMPORT_BYTES {
            return Err(ImportError::new(
                ImportErrorCode::InputTooLarge,
                "import content exceeds the size limit",
            ));
        }
        if document.trim().is_empty() {
            values.push(malformed_import_value());
            check_item_count(values.len())?;
            continue;
        }

        let entries = match parse_entries(document) {
            Ok((_, entries, _, _)) => entries,
            Err(error)
                if matches!(
                    error.code,
                    ImportErrorCode::EmptyInput | ImportErrorCode::MalformedJson
                ) =>
            {
                values.push(malformed_import_value());
                check_item_count(values.len())?;
                continue;
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            values.push(entry.value.unwrap_or_else(malformed_import_value));
            check_item_count(values.len())?;
        }
    }

    let combined = serde_json::to_string(&values).map_err(|_| {
        ImportError::new(
            ImportErrorCode::MalformedJson,
            "failed to combine import documents",
        )
    })?;
    if combined.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::new(
            ImportErrorCode::InputTooLarge,
            "import content exceeds the size limit",
        ));
    }
    Ok(combined)
}

pub(super) struct ParsedItem {
    pub(super) preview: ImportPreviewRow,
    pub(super) item: ParsedImportItem,
}

fn invalid_row(
    ordinal: usize,
    format: ImportFormat,
    source_file: Option<&str>,
    issue: ImportIssue,
) -> ImportPreviewRow {
    let seed = format!(
        "{}:{}:{:?}:{:?}",
        source_file.unwrap_or("pasted"),
        ordinal,
        format,
        issue.code
    );
    ImportPreviewRow {
        item_id: format!("import_{}", &sha256_hex(&seed, None, None)[..16]),
        source_file: source_file.map(redact_file_name),
        label: format!("Item {}", ordinal + 1),
        identity: "unknown".to_string(),
        auth_mode: ImportAuthMode::Unknown,
        source_name: format_name(format).to_string(),
        quota_status: ImportQuotaStatus::Skipped,
        status: ImportPreviewStatus::Invalid,
        plan: None,
        expires_at: None,
        subscription_expires_at: None,
        error: Some(issue),
        default_selected: false,
        selectable: false,
        existing: false,
        warnings: Vec::new(),
    }
}

fn validate_source_file(source_file: Option<&str>) -> Result<Option<String>, ImportError> {
    let Some(source_file) = source_file else {
        return Ok(None);
    };
    let source_file = source_file.trim();
    if source_file.is_empty()
        || source_file.len() > 128
        || source_file == "."
        || source_file == ".."
        || source_file.contains(['/', '\\'])
        || source_file.chars().any(char::is_control)
    {
        return Err(ImportError::new(
            ImportErrorCode::InvalidSourceFile,
            "source file name is unsafe",
        ));
    }
    Ok(Some(source_file.to_string()))
}

fn ensure_depth(root: &Value) -> Result<(), ImportError> {
    let mut stack = vec![(root, 1usize)];
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_JSON_DEPTH {
            return Err(ImportError::new(
                ImportErrorCode::JsonTooDeep,
                "import JSON exceeds the nesting limit",
            ));
        }
        match value {
            Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                stack.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_item_count(count: usize) -> Result<(), ImportError> {
    if count > MAX_IMPORT_ITEMS {
        Err(ImportError::new(
            ImportErrorCode::TooManyItems,
            "import content exceeds the item limit",
        ))
    } else {
        Ok(())
    }
}

fn format_name(value: ImportFormat) -> &'static str {
    match value {
        ImportFormat::JsonObject => "json_object",
        ImportFormat::JsonArray => "json_array",
        ImportFormat::JsonLines => "json_lines",
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
        ImportFormat::ZenithV1 => "zenith",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "access-super-secret";
    const REFRESH: &str = "refresh-super-secret";
    const ID: &str = "id-super-secret";
    const API_KEY: &str = "sk-super-secret";
    const EMAIL: &str = "private.user@example.test";

    #[test]
    fn combines_multiple_files_and_nested_account_containers() {
        let documents = vec![
            format!(
                r#"{{"account_id":"account-one","email":"one@example.test","access_token":"{ACCESS}"}}"#
            ),
            format!(
                r#"[{{"account_id":"account-two","email":"two@example.test","access_token":"{ACCESS}-two"}},{{"account_id":"account-three","email":"three@example.test","access_token":"{ACCESS}-three"}}]"#
            ),
            format!(
                r#"{{"accounts":[{{"account_id":"account-four","email":"four@example.test","access_token":"{ACCESS}-four"}}]}}"#
            ),
        ];

        let combined = combine_import_documents(&documents).unwrap();
        let parsed = parse_import(&combined, None, &[]).unwrap();

        assert_eq!(parsed.preview.format, ImportFormat::JsonArray);
        assert_eq!(parsed.preview.rows.len(), 4);
        assert_eq!(parsed.items.len(), 4);
        assert_eq!(
            parsed
                .items
                .iter()
                .map(|item| item.identity_key.as_str())
                .collect::<HashSet<_>>()
                .len(),
            4
        );
    }

    #[test]
    fn combines_valid_files_with_malformed_files_as_error_rows() {
        let documents = vec![
            format!(r#"{{"account_id":"account-one","access_token":"{ACCESS}"}}"#),
            r#"{"access_token":"truncated""#.to_string(),
            "Bearer header.payload.signature".to_string(),
        ];

        let combined = combine_import_documents(&documents).unwrap();
        let parsed = parse_import(&combined, None, &[]).unwrap();

        assert_eq!(parsed.preview.rows.len(), 3);
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.preview.rows[1].status, ImportPreviewStatus::Invalid);
        assert_eq!(
            parsed.preview.rows[1]
                .error
                .as_ref()
                .map(|error| error.code),
            Some(ImportIssueCode::MalformedJson)
        );
    }

    #[test]
    fn parses_raw_access_tokens_with_bearer_prefix_and_token_lines() {
        let input = "Bearer header.payload.signature\nat-opaque-token\n\"at-quoted-token\"";
        let parsed = parse_import(input, None, &[]).unwrap();

        assert_eq!(parsed.preview.format, ImportFormat::JsonLines);
        assert_eq!(parsed.items.len(), 3);
        assert_eq!(
            parsed.items[0].secrets().access_token(),
            Some("header.payload.signature")
        );
        assert_eq!(
            parsed.items[1].secrets().access_token(),
            Some("at-opaque-token")
        );
        assert_eq!(
            parsed.items[2].secrets().access_token(),
            Some("at-quoted-token")
        );
        let preview = serde_json::to_string(&parsed.preview).unwrap();
        assert!(!preview.contains("opaque-token"));

        let array =
            parse_import(r#"["at-array-token","Bearer one.two.three"]"#, None, &[]).unwrap();
        assert_eq!(array.items.len(), 2);
        assert_eq!(
            array.items[1].secrets().access_token(),
            Some("one.two.three")
        );
    }

    #[test]
    fn parses_nested_account_subscription_metadata_for_opaque_tokens() {
        let parsed = parse_import(
            r#"{
                "access_token":"at-private-token",
                "account":{
                    "id":"account-team",
                    "email":"team@example.test",
                    "planType":"team",
                    "subscriptionActiveUntil":"2026-10-19T14:17:45Z"
                }
            }"#,
            None,
            &[],
        )
        .unwrap();

        assert_eq!(parsed.preview.rows[0].plan.as_deref(), Some("team"));
        assert_eq!(
            parsed.preview.rows[0].subscription_expires_at.as_deref(),
            Some("2026-10-19T14:17:45Z")
        );
        assert_eq!(parsed.items[0].account_id.as_deref(), Some("account-team"));
        assert!(!serde_json::to_string(&parsed.preview)
            .unwrap()
            .contains("at-private-token"));
    }

    #[test]
    fn parses_codex_auth_json_token_and_api_key_shapes() {
        let oauth = parse_import(
            &format!(
                r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":"{API_KEY}","tokens":{{"access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}"}},"email":"{EMAIL}"}}"#
            ),
            Some("auth.json"),
            &[],
        )
        .unwrap();
        assert_eq!(oauth.preview.rows[0].auth_mode, ImportAuthMode::OAuth);
        assert_eq!(oauth.items[0].secrets().access_token(), Some(ACCESS));
        assert_eq!(oauth.items[0].secrets().refresh_token(), Some(REFRESH));
        assert_eq!(oauth.items[0].secrets().id_token(), Some(ID));
        assert_eq!(oauth.items[0].secrets().api_key(), None);
        assert_eq!(oauth.items[0].email(), Some(EMAIL));
        assert_ne!(oauth.preview.rows[0].identity, EMAIL);
        assert!(oauth.preview.rows[0]
            .warnings
            .iter()
            .any(|warning| { warning.code == ImportWarningCode::UnusedCredentialsIgnored }));

        let api_key = parse_import(
            &format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{API_KEY}"}}"#),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(api_key.preview.rows[0].auth_mode, ImportAuthMode::ApiKey);
        assert_eq!(api_key.items[0].secrets().api_key(), Some(API_KEY));
    }

    #[test]
    fn parses_top_level_nested_and_degraded_token_shapes() {
        let top_level = parse_import(
            &format!(
                r#"{{"access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}"}}"#
            ),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(
            top_level.preview.rows[0].auth_mode,
            ImportAuthMode::ImportedToken
        );

        let nested = parse_import(
            &format!(r#"{{"tokens":{{"accessToken":"{ACCESS}"}}}}"#),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(nested.items[0].secrets().access_token(), Some(ACCESS));
        assert!(nested.preview.rows[0]
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::AccessTokenOnly));

        let refresh_only =
            parse_import(&format!(r#"{{"refresh_token":"{REFRESH}"}}"#), None, &[]).unwrap();
        assert_eq!(
            refresh_only.items[0].secrets().refresh_token(),
            Some(REFRESH)
        );
        assert!(refresh_only.preview.rows[0]
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::RefreshExchangeRequired));
    }

    #[test]
    fn parses_api_key_metadata_array_and_json_lines() {
        let array = parse_import(
            &format!(
                r#"[{{"api_key":"{API_KEY}","base_url":"https://api.example.test/v1?discard=1","protocol":"responses"}},{{"access_token":"{ACCESS}"}}]"#
            ),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(array.preview.format, ImportFormat::JsonArray);
        assert_eq!(array.items.len(), 2);
        assert_eq!(
            array.items[0].base_url.as_deref(),
            Some("https://api.example.test/v1")
        );
        assert_eq!(array.items[0].protocol.as_deref(), Some("responses"));

        let json_lines = parse_import(
            &format!(
                "{{\"access_token\":\"{ACCESS}\"}}\nnot-json-{API_KEY}\n{{\"refresh_token\":\"{REFRESH}\"}}"
            ),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(json_lines.preview.format, ImportFormat::JsonLines);
        assert_eq!(json_lines.preview.rows.len(), 3);
        assert_eq!(json_lines.items.len(), 2);
        assert_eq!(
            json_lines.preview.rows[1].status,
            ImportPreviewStatus::Invalid
        );
    }

    #[test]
    fn portable_bundle_is_neutral_and_never_imports_proxies() {
        let input = format!(
            r#"{{
                "type":"sb2api",
                "version":1,
                "exported_at":"2026-07-10T00:00:00Z",
                "proxies":[{{"url":"http://proxy-secret.test"}},{{"url":"http://other.test"}}],
                "accounts":[{{
                    "name":"Personal account",
                    "type":"chatgpt",
                    "platform":"openai",
                    "priority":7,
                    "concurrency":3,
                    "credentials":{{
                        "access_token":"{ACCESS}",
                        "refresh_token":"{REFRESH}",
                        "id_token":"{ID}",
                        "expires_at":1783692000,
                        "email":"{EMAIL}",
                        "chatgpt_account_id":"acct_1234567890",
                        "chatgpt_user_id":"user_1234567890",
                        "organization_id":"org_1234567890",
                        "plan_type":"plus",
                        "subscription_expires_at":"2026-08-10T00:00:00Z"
                    }}
                }}]
            }}"#
        );
        let parsed = parse_import(&input, Some("portable.json"), &[]).unwrap();
        assert_eq!(parsed.preview.format, ImportFormat::PortableAccountBundleV1);
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].priority, Some(7));
        assert_eq!(parsed.preview.warnings.len(), 1);
        assert_eq!(
            parsed.preview.warnings[0],
            ImportWarning::count(ImportWarningCode::ProxiesIgnored, 2)
        );
        assert!(parsed.preview.rows[0]
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::ConcurrencyIgnored));
        let preview = serde_json::to_string(&parsed.preview).unwrap();
        assert!(!preview.to_ascii_lowercase().contains("sb2api"));
        assert!(!preview.contains("proxy-secret"));
    }

    #[test]
    fn zenith_bundle_preserves_description_and_nested_account_data() {
        let input = format!(
            r#"{{
                "format":"zenith",
                "version":1,
                "exportedAt":"2026-07-19T00:00:00Z",
                "description":"Seller description",
                "accounts":[{{
                    "name":"Business account",
                    "provider":"openai",
                    "auth":{{
                        "type":"oauth",
                        "accessToken":"{ACCESS}",
                        "refreshToken":"{REFRESH}",
                        "idToken":"{ID}",
                        "expiresAt":"2026-08-19T00:00:00Z"
                    }},
                    "identity":{{
                        "email":"{EMAIL}",
                        "accountId":"acct_zenith",
                        "userId":"user_zenith",
                        "organizationId":"org_zenith"
                    }},
                    "subscription":{{
                        "plan":"business",
                        "expiresAt":"2026-09-19T00:00:00Z"
                    }}
                }}]
            }}"#
        );
        let parsed = parse_import(&input, Some("zenith-accounts.json"), &[]).unwrap();

        assert_eq!(parsed.preview.format, ImportFormat::ZenithV1);
        assert_eq!(
            parsed.preview.description.as_deref(),
            Some("Seller description")
        );
        assert_eq!(parsed.preview.rows[0].source_name, "zenith");
        assert_eq!(parsed.preview.rows[0].plan.as_deref(), Some("business"));
        assert_eq!(
            parsed.preview.rows[0].subscription_expires_at.as_deref(),
            Some("2026-09-19T00:00:00Z")
        );
        assert_eq!(parsed.items[0].secrets().access_token(), Some(ACCESS));
        assert_eq!(parsed.items[0].secrets().refresh_token(), Some(REFRESH));
        assert_eq!(parsed.items[0].account_id.as_deref(), Some("acct_zenith"));
        assert_eq!(
            parsed.items[0].chatgpt_user_id.as_deref(),
            Some("user_zenith")
        );
        assert_eq!(
            parsed.items[0].organization_id.as_deref(),
            Some("org_zenith")
        );

        let unsupported = input.replacen("\"version\":1", "\"version\":2", 1);
        assert_eq!(
            parse_import(&unsupported, None, &[]).unwrap_err().code,
            ImportErrorCode::UnsupportedBundleVersion
        );
    }

    #[test]
    fn parses_current_public_account_export_shapes() {
        let fixtures = [
            format!(
                r#"{{"type":"codex","access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}","email":"{EMAIL}","account_id":"acct_cpa","plan_type":"plus","expired":"2026-08-10T00:00:00Z"}}"#
            ),
            format!(
                r#"{{"exported_at":"2026-07-10T00:00:00Z","proxies":[],"accounts":[{{"name":"sub2api account","platform":"openai","type":"oauth","credentials":{{"access_token":"{ACCESS}","email":"{EMAIL}","chatgpt_account_id":"acct_sub2api","plan_type":"plus"}}}}]}}"#
            ),
            format!(
                r#"{{"type":"codex","id_token":"{ID}","access_token":"{ACCESS}","refresh_token":"{REFRESH}","account_id":"acct_cockpit","email":"{EMAIL}","expired":"2026-08-10T00:00:00Z"}}"#
            ),
            format!(
                r#"{{"accessToken":"{ACCESS}","refreshToken":"{REFRESH}","email":"{EMAIL}","name":"9router account","authType":"oauth","providerSpecificData":{{"chatgptAccountId":"acct_9router","chatgptPlanType":"plus"}}}}"#
            ),
            format!(
                r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"{ID}","access_token":"{ACCESS}","refresh_token":"{REFRESH}","account_id":"acct_codex"}},"last_refresh":"2026-07-10T00:00:00Z"}}"#
            ),
            format!(
                r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{ACCESS}","refresh_token":"__missing_refresh_token__","id_token":"{ID}"}},"last_refresh":"2026-07-10T00:00:00Z"}}"#
            ),
            format!(
                r#"{{"tokens":{{"access_token":"{ACCESS}","refresh_token":"","id_token":"","chatgpt_account_id":"acct_manager"}},"meta":{{"label":"Manager account","workspace_id":"workspace_1","chatgpt_account_id":"acct_manager"}}}}"#
            ),
        ];

        for (index, fixture) in fixtures.iter().enumerate() {
            let parsed = parse_import(fixture, None, &[])
                .unwrap_or_else(|error| panic!("fixture {index} failed: {error}"));
            assert_eq!(parsed.items.len(), 1, "fixture {index}");
            assert_eq!(
                parsed.items[0].secrets().access_token(),
                Some(ACCESS),
                "fixture {index}"
            );
            let preview = serde_json::to_string(&parsed.preview).unwrap();
            for secret in [ACCESS, REFRESH, ID, EMAIL] {
                assert!(!preview.contains(secret), "fixture {index}");
            }
        }

        let nine_router = parse_import(&fixtures[3], None, &[]).unwrap();
        assert_eq!(nine_router.preview.format, ImportFormat::JsonObject);
        assert_eq!(
            nine_router.items[0].account_id.as_deref(),
            Some("acct_9router")
        );
        assert_eq!(nine_router.preview.rows[0].plan.as_deref(), Some("plus"));
        let wrapped_nine_router =
            parse_import(&format!(r#"{{"accounts":[{}]}}"#, fixtures[3]), None, &[]).unwrap();
        assert_eq!(wrapped_nine_router.preview.format, ImportFormat::JsonArray);
        assert_eq!(wrapped_nine_router.items.len(), 1);

        let axon_hub = parse_import(&fixtures[5], None, &[]).unwrap();
        assert_eq!(axon_hub.items[0].secrets().refresh_token(), None);
        assert!(axon_hub.preview.rows[0]
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::AccessTokenOnly));

        let manager = parse_import(&fixtures[6], None, &[]).unwrap();
        assert_eq!(manager.preview.rows[0].label, "Manager account");
        assert_eq!(manager.items[0].account_id.as_deref(), Some("acct_manager"));
        for access_only in [parse_import(&fixtures[1], None, &[]).unwrap(), manager] {
            assert_eq!(access_only.items[0].secrets().refresh_token(), None);
            assert!(access_only.preview.rows[0]
                .warnings
                .iter()
                .any(|warning| warning.code == ImportWarningCode::AccessTokenOnly));
        }
    }

    #[test]
    fn parses_mixed_cockpit_array_and_sub2api_failure_rows() {
        let cockpit = format!(
            r#"[
                {{"type":"codex","access_token":"{ACCESS}-one","refresh_token":"{REFRESH}-one","account_id":"acct_cockpit_one","email":"one@example.test"}},
                {{"type":"codex","access_token":"{ACCESS}-two","account_id":"acct_cockpit_two","email":"two@example.test"}},
                {{"auth_mode":"apikey","OPENAI_API_KEY":"{API_KEY}","api_base_url":"https://api.example.test/v1","api_provider_name":"Example API"}}
            ]"#
        );
        let parsed = parse_import(&cockpit, None, &[]).unwrap();
        assert_eq!(parsed.preview.format, ImportFormat::JsonArray);
        assert_eq!(parsed.items.len(), 3);
        assert!(parsed.preview.rows.iter().all(|row| row.selectable));
        assert_eq!(parsed.items[2].label, "Example API");
        assert_eq!(
            parsed.items[2].base_url.as_deref(),
            Some("https://api.example.test/v1")
        );

        let sub2api = format!(
            r#"{{"type":"sub2api-data","version":1,"accounts":[
                {{"name":"First","platform":"openai","type":"oauth","credentials":{{"access_token":"{ACCESS}-one","chatgpt_account_id":"acct_sub2api_one","email":"one@example.test"}}}},
                {{"name":"Second","platform":"openai","type":"oauth","credentials":{{"access_token":"{ACCESS}-two","chatgpt_account_id":"acct_sub2api_two","email":"two@example.test"}}}},
                {{"name":"Missing credential","platform":"openai","type":"oauth","credentials":{{"access_token":"","email":"missing@example.test"}}}}
            ],"proxies":[]}}"#
        );
        let parsed = parse_import(&sub2api, None, &[]).unwrap();
        assert_eq!(parsed.preview.format, ImportFormat::PortableAccountBundleV1);
        assert_eq!(parsed.preview.rows.len(), 3);
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.preview.rows[2].status, ImportPreviewStatus::Invalid);
        assert_eq!(
            parsed.preview.rows[2]
                .error
                .as_ref()
                .map(|error| error.code),
            Some(ImportIssueCode::MissingCredentials)
        );
    }

    #[test]
    fn parses_sub2api_agent_identity_accounts() {
        let input = serde_json::json!({
            "type": "sub2api-data",
            "version": 1,
            "accounts": [{
                "name": "Agent account",
                "platform": "openai",
                "type": "oauth",
                "credentials": {
                    "auth_mode": "agentIdentity",
                    "agent_private_key": "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g",
                    "agent_runtime_id": "runtime-test",
                    "task_id": "task-test",
                    "chatgpt_account_id": "account-test",
                    "email": "agent@example.test"
                }
            }]
        })
        .to_string();
        let parsed = parse_import(&input, Some("sub2api.json"), &[]).unwrap();

        assert_eq!(parsed.items.len(), 1);
        assert_eq!(
            parsed.preview.rows[0].auth_mode,
            ImportAuthMode::AgentIdentity
        );
        assert_eq!(parsed.preview.rows[0].status, ImportPreviewStatus::Ready);
        assert_eq!(parsed.items[0].account_id.as_deref(), Some("account-test"));
        assert!(parsed.items[0].secrets().access_token().is_none());
        assert!(parsed.items[0].secrets().agent_private_key().is_some());
    }

    #[test]
    fn duplicates_and_existing_items_have_safe_selection_states() {
        let input = format!(
            r#"[{{"email":"{EMAIL}","access_token":"{ACCESS}"}},{{"email":"{EMAIL}","access_token":"different-secret"}}]"#
        );
        let first = parse_import(&input, None, &[]).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.preview.rows[1].status, ImportPreviewStatus::Invalid);
        assert_eq!(
            first.preview.rows[1].error.as_ref().map(|error| error.code),
            Some(ImportIssueCode::DuplicateItem)
        );

        let existing_key = first.items[0].identity_key.clone();
        let existing = parse_import(
            &format!(r#"{{"email":"{EMAIL}","access_token":"{ACCESS}"}}"#),
            None,
            &[existing_key],
        )
        .unwrap();
        assert_eq!(
            existing.preview.rows[0].status,
            ImportPreviewStatus::Existing
        );
        assert!(existing.preview.rows[0].selectable);
        assert!(!existing.preview.rows[0].default_selected);
        assert!(existing.preview.rows[0].existing);

        let different_kinds = parse_import(
            &format!(
                r#"[{{"auth_mode":"apikey","email":"{EMAIL}","OPENAI_API_KEY":"{API_KEY}"}},{{"auth_mode":"chatgpt","email":"{EMAIL}","access_token":"{ACCESS}"}}]"#
            ),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(different_kinds.items.len(), 2);
        assert_ne!(
            different_kinds.preview.rows[0].item_id,
            different_kinds.preview.rows[1].item_id
        );
    }

    #[test]
    fn shared_team_account_id_does_not_merge_distinct_users() {
        let input = format!(
            r#"[{{"account_id":"shared-team","email":"one@example.test","access_token":"{ACCESS}"}},{{"account_id":"shared-team","email":"two@example.test","access_token":"different-secret"}}]"#
        );
        let parsed = parse_import(&input, None, &[]).unwrap();
        assert_eq!(parsed.items.len(), 2);
        assert_ne!(parsed.items[0].identity_key, parsed.items[1].identity_key);
        assert!(parsed.preview.rows.iter().all(|row| row.selectable));
    }

    #[test]
    fn limits_and_malformed_input_return_redacted_errors() {
        let oversized = "x".repeat(MAX_IMPORT_BYTES + 1);
        assert_eq!(
            parse_import(&oversized, None, &[]).unwrap_err().code,
            ImportErrorCode::InputTooLarge
        );

        let too_many = format!(
            "[{}]",
            std::iter::repeat_n(
                format!(r#"{{"access_token":"{ACCESS}"}}"#),
                MAX_IMPORT_ITEMS + 1
            )
            .collect::<Vec<_>>()
            .join(",")
        );
        assert_eq!(
            parse_import(&too_many, None, &[]).unwrap_err().code,
            ImportErrorCode::TooManyItems
        );

        let mut deep = format!(r#"{{"access_token":"{ACCESS}","deep":"#);
        deep.push_str(&"[".repeat(MAX_JSON_DEPTH + 1));
        deep.push_str("null");
        deep.push_str(&"]".repeat(MAX_JSON_DEPTH + 1));
        deep.push('}');
        assert_eq!(
            parse_import(&deep, None, &[]).unwrap_err().code,
            ImportErrorCode::JsonTooDeep
        );

        let malformed = format!(r#"{{"access_token":"{ACCESS}""#);
        let error = parse_import(&malformed, None, &[]).unwrap_err();
        let serialized = serde_json::to_string(&error).unwrap();
        assert_eq!(error.code, ImportErrorCode::MalformedJson);
        assert!(!serialized.contains(ACCESS));
    }

    #[test]
    fn previews_errors_and_debug_output_never_contain_fixture_secrets() {
        let input = format!(
            r#"{{"name":"{ACCESS}","email":"{EMAIL}","access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}","plan_type":"{ACCESS}","expires_at":"{REFRESH}"}}"#
        );
        let parsed = parse_import(&input, Some("access-super-secret.json"), &[]).unwrap();
        let preview = serde_json::to_string(&parsed.preview).unwrap();
        let debug = format!("{parsed:?} {:?}", parsed.items[0].secrets());
        for secret in [ACCESS, REFRESH, ID, API_KEY, EMAIL] {
            assert!(!preview.contains(secret));
            assert!(!debug.contains(secret));
        }
        assert!(debug.contains("[redacted]"));

        let invalid = parse_import(
            &format!(r#"{{"api_key":"{API_KEY}","access_token":"{ACCESS}"}}"#),
            None,
            &[],
        )
        .unwrap();
        let invalid_preview = serde_json::to_string(&invalid.preview).unwrap();
        assert!(!invalid_preview.contains(API_KEY));
        assert!(!invalid_preview.contains(ACCESS));
    }

    #[test]
    fn unsafe_source_names_and_unsupported_bundle_versions_are_rejected() {
        let input = format!(r#"{{"access_token":"{ACCESS}"}}"#);
        assert_eq!(
            parse_import(&input, Some("../auth.json"), &[])
                .unwrap_err()
                .code,
            ImportErrorCode::InvalidSourceFile
        );
        assert_eq!(
            parse_import(
                r#"{"type":"portable_account_bundle","version":2,"accounts":[]}"#,
                None,
                &[],
            )
            .unwrap_err()
            .code,
            ImportErrorCode::UnsupportedBundleVersion
        );
    }
}

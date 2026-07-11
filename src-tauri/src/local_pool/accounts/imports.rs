use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fmt};
use url::Url;

pub const MAX_IMPORT_BYTES: usize = 1024 * 1024;
pub const MAX_IMPORT_ITEMS: usize = 256;
pub const MAX_JSON_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    JsonObject,
    JsonArray,
    JsonLines,
    PortableAccountBundleV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportAuthMode {
    OAuth,
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
    let (format, entries, warnings) = parse_entries(input)?;
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
        if document.trim().is_empty() {
            return Err(ImportError::new(
                ImportErrorCode::EmptyInput,
                "import content is empty",
            ));
        }
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

        let (_, entries, _) = parse_entries(document)?;
        for entry in entries {
            let Some(value) = entry.value else {
                return Err(ImportError::new(
                    ImportErrorCode::MalformedJson,
                    "import JSON is malformed",
                ));
            };
            values.push(value);
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

struct InputEntry {
    ordinal: usize,
    value: Option<Value>,
    issue: Option<ImportIssue>,
}

struct ParsedItem {
    preview: ImportPreviewRow,
    item: ParsedImportItem,
}

fn parse_entries(
    input: &str,
) -> Result<(ImportFormat, Vec<InputEntry>, Vec<ImportWarning>), ImportError> {
    match serde_json::from_str::<Value>(input) {
        Ok(value) => {
            ensure_depth(&value)?;
            if let Some(object) = value.as_object() {
                if is_portable_bundle(object) {
                    return parse_portable_bundle(object);
                }
                if object.get("accounts").is_some_and(Value::is_array) {
                    return parse_account_container(object);
                }
                return Ok((
                    ImportFormat::JsonObject,
                    vec![InputEntry {
                        ordinal: 0,
                        value: Some(value),
                        issue: None,
                    }],
                    Vec::new(),
                ));
            }
            if let Some(values) = value.as_array() {
                check_item_count(values.len())?;
                let entries = values
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(ordinal, value)| InputEntry {
                        ordinal,
                        value: Some(value),
                        issue: None,
                    })
                    .collect();
                return Ok((ImportFormat::JsonArray, entries, Vec::new()));
            }
            Ok((
                ImportFormat::JsonObject,
                vec![InputEntry {
                    ordinal: 0,
                    value: Some(value),
                    issue: None,
                }],
                Vec::new(),
            ))
        }
        Err(_) => parse_json_lines(input),
    }
}

fn parse_json_lines(
    input: &str,
) -> Result<(ImportFormat, Vec<InputEntry>, Vec<ImportWarning>), ImportError> {
    let lines = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.len() <= 1 {
        return Err(ImportError::new(
            ImportErrorCode::MalformedJson,
            "import content is not valid JSON",
        ));
    }
    check_item_count(lines.len())?;

    let mut entries = Vec::with_capacity(lines.len());
    for (ordinal, line) in lines.into_iter().enumerate() {
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                ensure_depth(&value)?;
                entries.push(InputEntry {
                    ordinal,
                    value: Some(value),
                    issue: None,
                });
            }
            Err(_) => entries.push(InputEntry {
                ordinal,
                value: None,
                issue: Some(ImportIssue::new(
                    ImportIssueCode::MalformedJson,
                    "malformed JSON line",
                )),
            }),
        }
    }
    Ok((ImportFormat::JsonLines, entries, Vec::new()))
}

fn is_portable_bundle(object: &Map<String, Value>) -> bool {
    let Some(accounts) = object.get("accounts").and_then(Value::as_array) else {
        return false;
    };
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("portable_account_bundle"))
        || accounts.iter().any(|account| {
            account
                .as_object()
                .is_some_and(|account| account.get("credentials").is_some_and(Value::is_object))
        })
}

fn parse_portable_bundle(
    object: &Map<String, Value>,
) -> Result<(ImportFormat, Vec<InputEntry>, Vec<ImportWarning>), ImportError> {
    let version = object.get("version").and_then(bundle_version).unwrap_or(1);
    if version != 1 {
        return Err(ImportError::new(
            ImportErrorCode::UnsupportedBundleVersion,
            "portable account bundle version is unsupported",
        ));
    }
    let accounts = object
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::MalformedJson,
                "portable account bundle has no account list",
            )
        })?;
    check_item_count(accounts.len())?;
    let entries = accounts
        .iter()
        .cloned()
        .enumerate()
        .map(|(ordinal, value)| InputEntry {
            ordinal,
            value: Some(value),
            issue: None,
        })
        .collect();
    let proxy_count = object.get("proxies").map(container_count).unwrap_or(0);
    let warnings = (proxy_count > 0)
        .then(|| ImportWarning::count(ImportWarningCode::ProxiesIgnored, proxy_count))
        .into_iter()
        .collect();
    Ok((ImportFormat::PortableAccountBundleV1, entries, warnings))
}

fn parse_account_container(
    object: &Map<String, Value>,
) -> Result<(ImportFormat, Vec<InputEntry>, Vec<ImportWarning>), ImportError> {
    let accounts = object
        .get("accounts")
        .and_then(Value::as_array)
        .expect("account container checked by caller");
    check_item_count(accounts.len())?;
    let entries = accounts
        .iter()
        .cloned()
        .enumerate()
        .map(|(ordinal, value)| InputEntry {
            ordinal,
            value: Some(value),
            issue: None,
        })
        .collect();
    let proxy_count = object.get("proxies").map(container_count).unwrap_or(0);
    let warnings = (proxy_count > 0)
        .then(|| ImportWarning::count(ImportWarningCode::ProxiesIgnored, proxy_count))
        .into_iter()
        .collect();
    Ok((ImportFormat::JsonArray, entries, warnings))
}

fn bundle_version(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn container_count(value: &Value) -> usize {
    match value {
        Value::Array(values) => values.len(),
        Value::Object(values) => values.len(),
        Value::Null => 0,
        _ => 1,
    }
}

fn parse_item(
    value: &Value,
    ordinal: usize,
    format: ImportFormat,
    source_file: Option<&str>,
) -> Result<ParsedItem, ImportIssue> {
    let object = value.as_object().ok_or_else(|| {
        ImportIssue::new(
            ImportIssueCode::UnsupportedValue,
            "import item must be a JSON object",
        )
    })?;
    let credentials = object
        .get("credentials")
        .and_then(Value::as_object)
        .unwrap_or(object);
    let provider_data = object
        .get("providerSpecificData")
        .or_else(|| object.get("provider_specific_data"))
        .and_then(Value::as_object);
    let meta = object.get("meta").and_then(Value::as_object);
    let tokens = object
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| credentials.get("tokens").and_then(Value::as_object));

    let api_key = credential_string(object, credentials, tokens, API_KEY_FIELDS);
    let access_token = credential_string(object, credentials, tokens, ACCESS_TOKEN_FIELDS);
    let refresh_token = credential_string(object, credentials, tokens, REFRESH_TOKEN_FIELDS)
        .filter(|value| value != "__missing_refresh_token__");
    let id_token = credential_string(object, credentials, tokens, ID_TOKEN_FIELDS);
    let has_api_key = api_key.is_some();
    let has_tokens = access_token.is_some() || refresh_token.is_some() || id_token.is_some();
    let explicit_auth_mode = string_field(object, &["auth_mode", "authMode", "authType"]);
    let explicit_oauth = explicit_auth_mode.is_some_and(is_oauth_mode);
    let explicit_api_key = explicit_auth_mode.is_some_and(is_api_key_mode);
    let mut warnings = Vec::new();
    let (use_api_key, use_tokens) = if has_api_key && has_tokens {
        if explicit_oauth {
            warnings.push(ImportWarning::new(
                ImportWarningCode::UnusedCredentialsIgnored,
            ));
            (false, true)
        } else if explicit_api_key {
            warnings.push(ImportWarning::new(
                ImportWarningCode::UnusedCredentialsIgnored,
            ));
            (true, false)
        } else {
            return Err(ImportIssue::new(
                ImportIssueCode::AmbiguousCredentials,
                "import item contains multiple credential kinds",
            ));
        }
    } else {
        (has_api_key, has_tokens)
    };
    if !has_api_key && !has_tokens {
        return Err(ImportIssue::new(
            ImportIssueCode::MissingCredentials,
            "import item has no supported credential",
        ));
    }
    if use_tokens && access_token.is_none() && refresh_token.is_none() {
        return Err(ImportIssue::new(
            ImportIssueCode::InvalidCredentials,
            "token import requires an access or refresh token",
        ));
    }

    let auth_mode = if use_api_key {
        ImportAuthMode::ApiKey
    } else if explicit_oauth {
        ImportAuthMode::OAuth
    } else {
        if explicit_auth_mode.is_some_and(|mode| !is_token_mode(mode)) {
            warnings.push(ImportWarning::new(ImportWarningCode::UnknownAuthMode));
        }
        ImportAuthMode::ImportedToken
    };
    if use_tokens && access_token.is_some() && refresh_token.is_none() {
        warnings.push(ImportWarning::new(ImportWarningCode::AccessTokenOnly));
    }
    if use_tokens && access_token.is_none() && refresh_token.is_some() {
        warnings.push(ImportWarning::new(
            ImportWarningCode::RefreshExchangeRequired,
        ));
    }
    if object.contains_key("concurrency") {
        warnings.push(ImportWarning::new(ImportWarningCode::ConcurrencyIgnored));
    }

    let email = credential_string(object, credentials, None, EMAIL_FIELDS).or_else(|| {
        provider_data
            .and_then(|data| string_field(data, EMAIL_FIELDS))
            .map(str::to_string)
    });
    let account_id_value = credential_str(object, credentials, None, ACCOUNT_ID_FIELDS)
        .or_else(|| provider_data.and_then(|data| string_field(data, ACCOUNT_ID_FIELDS)))
        .or_else(|| meta.and_then(|data| string_field(data, ACCOUNT_ID_FIELDS)));
    let account_id = safe_identifier(account_id_value);
    let chatgpt_user_id_value = credential_str(object, credentials, None, USER_ID_FIELDS)
        .or_else(|| provider_data.and_then(|data| string_field(data, USER_ID_FIELDS)))
        .or_else(|| meta.and_then(|data| string_field(data, USER_ID_FIELDS)));
    let chatgpt_user_id = safe_identifier(chatgpt_user_id_value);
    let organization_id_value = credential_str(object, credentials, None, ORGANIZATION_ID_FIELDS)
        .or_else(|| provider_data.and_then(|data| string_field(data, ORGANIZATION_ID_FIELDS)))
        .or_else(|| meta.and_then(|data| string_field(data, ORGANIZATION_ID_FIELDS)));
    let organization_id = safe_identifier(organization_id_value);
    let plan_value = credential_value(object, credentials, None, PLAN_FIELDS)
        .or_else(|| provider_data.and_then(|data| value_field(data, PLAN_FIELDS)))
        .or_else(|| meta.and_then(|data| value_field(data, PLAN_FIELDS)));
    let mut plan = safe_metadata(plan_value.and_then(Value::as_str));
    let expires_at_value = credential_value(object, credentials, None, EXPIRES_AT_FIELDS)
        .or_else(|| provider_data.and_then(|data| value_field(data, EXPIRES_AT_FIELDS)));
    let mut expires_at = safe_expiry(expires_at_value);
    let subscription_expires_at_value =
        credential_value(object, credentials, None, SUBSCRIPTION_EXPIRES_AT_FIELDS).or_else(|| {
            provider_data.and_then(|data| value_field(data, SUBSCRIPTION_EXPIRES_AT_FIELDS))
        });
    let mut subscription_expires_at = safe_expiry(subscription_expires_at_value);
    let base_url_value = value_field(object, &["base_url", "baseUrl", "api_base", "apiBase"]);
    let base_url_supplied = base_url_value.is_some();
    let base_url = safe_base_url(base_url_value.and_then(Value::as_str));
    let protocol_value = value_field(object, &["protocol", "wire_api", "wireApi"]);
    let protocol_supplied = protocol_value.is_some();
    let protocol = safe_protocol(protocol_value.and_then(Value::as_str));
    let priority = object
        .get("priority")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    if metadata_was_rejected(
        object,
        base_url.as_deref(),
        protocol.as_deref(),
        plan_value,
        plan.as_deref(),
    ) {
        warnings.push(ImportWarning::new(
            ImportWarningCode::InvalidMetadataIgnored,
        ));
    }

    let email_value = email.as_deref();
    let identity_seed = if use_api_key {
        account_id
            .as_deref()
            .map(|value| format!("account:{}", value.to_ascii_lowercase()))
            .or_else(|| {
                email_value.map(|value| format!("email:{}", value.trim().to_ascii_lowercase()))
            })
    } else {
        token_identity_seed(
            account_id.as_deref(),
            chatgpt_user_id.as_deref(),
            email_value,
        )
    };
    let credential_fingerprint = if use_api_key {
        api_key.as_deref()
    } else {
        refresh_token
            .as_deref()
            .or(access_token.as_deref())
            .or(id_token.as_deref())
    }
    .expect("selected credential presence checked");
    let identity_key = if let Some(identity_seed) = identity_seed.as_deref() {
        let identity_seed = if use_api_key {
            format!(
                "api:{}:{identity_seed}",
                base_url.as_deref().unwrap_or("default")
            )
        } else {
            format!("account:{identity_seed}")
        };
        sha256_hex(&identity_seed, None, None)
    } else {
        sha256_hex(
            if use_api_key {
                "api-key-without-identity"
            } else {
                "token-without-identity"
            },
            Some(credential_fingerprint),
            base_url.as_deref(),
        )
    };
    let item_seed = identity_seed
        .map(|identity_seed| {
            if use_api_key {
                format!(
                    "api:{}:{identity_seed}",
                    base_url.as_deref().unwrap_or("default")
                )
            } else {
                format!("account:{identity_seed}")
            }
        })
        .unwrap_or_else(|| {
            format!(
                "{}:{}:{}",
                source_file.unwrap_or("pasted"),
                ordinal,
                auth_mode_name(auth_mode)
            )
        });
    let item_id = format!("import_{}", &sha256_hex(&item_seed, None, None)[..16]);
    let identity = email_value
        .map(mask_email)
        .or_else(|| account_id.as_deref().map(mask_identifier))
        .unwrap_or_else(|| format!("imported-{}", ordinal + 1));
    let fallback_label = match auth_mode {
        ImportAuthMode::ApiKey => format!("API source {}", ordinal + 1),
        _ => format!("Account {}", ordinal + 1),
    };
    let label_value = string_field(object, &["name", "label"])
        .or_else(|| meta.and_then(|data| string_field(data, &["name", "label"])));
    let mut label = safe_label(label_value).unwrap_or_else(|| identity.clone());
    if label == "unknown" || label.is_empty() {
        label = fallback_label;
    }
    label = redact_label_secrets(
        label,
        [
            api_key.as_deref(),
            access_token.as_deref(),
            refresh_token.as_deref(),
            id_token.as_deref(),
            email_value,
        ],
        &identity,
    );
    let sensitive_values = [
        api_key.as_deref(),
        access_token.as_deref(),
        refresh_token.as_deref(),
        id_token.as_deref(),
        email_value,
    ];
    redact_optional_metadata(&mut plan, &sensitive_values);
    redact_optional_metadata(&mut expires_at, &sensitive_values);
    redact_optional_metadata(&mut subscription_expires_at, &sensitive_values);
    let preview_source_file =
        source_file.map(|source_file| redact_file_name_with(source_file, &sensitive_values));

    let source_name = match format {
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
        _ if explicit_auth_mode.is_some() => "codex_auth_json",
        _ if use_api_key => "api_key_json",
        _ => "token_json",
    }
    .to_string();
    let secrets = ImportSecretMaterial {
        access_token: if use_tokens {
            access_token.map(RedactedValue::new)
        } else {
            None
        },
        refresh_token: if use_tokens {
            refresh_token.map(RedactedValue::new)
        } else {
            None
        },
        id_token: if use_tokens {
            id_token.map(RedactedValue::new)
        } else {
            None
        },
        api_key: if use_api_key {
            api_key.map(RedactedValue::new)
        } else {
            None
        },
    };
    let preview = ImportPreviewRow {
        item_id: item_id.clone(),
        source_file: preview_source_file,
        label: label.clone(),
        identity,
        auth_mode,
        source_name,
        quota_status: ImportQuotaStatus::Skipped,
        status: ImportPreviewStatus::Ready,
        plan,
        expires_at,
        subscription_expires_at,
        error: None,
        default_selected: true,
        selectable: true,
        existing: false,
        warnings,
    };
    let item = ParsedImportItem {
        item_id,
        identity_key,
        label,
        account_id,
        chatgpt_user_id,
        organization_id,
        base_url,
        base_url_supplied,
        protocol,
        protocol_supplied,
        priority,
        email: email.map(RedactedValue::new),
        secrets,
    };
    Ok(ParsedItem { preview, item })
}

fn token_identity_seed(
    account_id: Option<&str>,
    user_id: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    let account = account_id.map(|value| value.trim().to_ascii_lowercase());
    let user = user_id.map(|value| value.trim().to_ascii_lowercase());
    let email = email.map(|value| value.trim().to_ascii_lowercase());
    match (account, email, user) {
        (Some(account), Some(email), _) => Some(format!("account:{account}:email:{email}")),
        (Some(account), None, Some(user)) => Some(format!("account:{account}:user:{user}")),
        (Some(account), None, None) => Some(format!("account:{account}")),
        (None, Some(email), _) => Some(format!("email:{email}")),
        (None, None, Some(user)) => Some(format!("user:{user}")),
        (None, None, None) => None,
    }
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

fn credential_string(
    object: &Map<String, Value>,
    credentials: &Map<String, Value>,
    tokens: Option<&Map<String, Value>>,
    fields: &[&str],
) -> Option<String> {
    credential_str(object, credentials, tokens, fields)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn credential_str<'a>(
    object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
    tokens: Option<&'a Map<String, Value>>,
    fields: &[&str],
) -> Option<&'a str> {
    tokens
        .and_then(|tokens| string_field(tokens, fields))
        .or_else(|| string_field(credentials, fields))
        .or_else(|| string_field(object, fields))
}

fn credential_value<'a>(
    object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
    tokens: Option<&'a Map<String, Value>>,
    fields: &[&str],
) -> Option<&'a Value> {
    tokens
        .and_then(|tokens| value_field(tokens, fields))
        .or_else(|| value_field(credentials, fields))
        .or_else(|| value_field(object, fields))
}

fn string_field<'a>(object: &'a Map<String, Value>, fields: &[&str]) -> Option<&'a str> {
    value_field(object, fields)?.as_str()
}

fn value_field<'a>(object: &'a Map<String, Value>, fields: &[&str]) -> Option<&'a Value> {
    fields.iter().find_map(|field| object.get(*field))
}

fn safe_identifier(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        None
    } else {
        Some(value.to_string())
    }
}

fn safe_metadata(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b' '))
    {
        None
    } else {
        Some(value.to_string())
    }
}

fn safe_expiry(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(value) => Some(value.to_string()),
        Value::String(value)
            if !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'+' | b'.' | b' ')
                }) =>
        {
            Some(value.to_string())
        }
        _ => None,
    }
}

fn safe_base_url(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.len() > 2048 {
        return None;
    }
    let mut parsed = Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.to_string().trim_end_matches('/').to_string())
}

fn safe_protocol(value: Option<&str>) -> Option<String> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "responses" => Some("responses".to_string()),
        "chat_completions" | "chat-completions" | "chat" => Some("chat_completions".to_string()),
        _ => None,
    }
}

fn metadata_was_rejected(
    object: &Map<String, Value>,
    base_url: Option<&str>,
    protocol: Option<&str>,
    plan_value: Option<&Value>,
    plan: Option<&str>,
) -> bool {
    (value_field(object, &["base_url", "baseUrl", "api_base", "apiBase"]).is_some()
        && base_url.is_none())
        || (value_field(object, &["protocol", "wire_api", "wireApi"]).is_some()
            && protocol.is_none())
        || (plan_value.is_some() && plan.is_none())
}

fn safe_label(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return None;
    }
    Some(if value.contains('@') {
        mask_email(value)
    } else {
        value.to_string()
    })
}

fn redact_label_secrets<'a>(
    label: String,
    secrets: impl IntoIterator<Item = Option<&'a str>>,
    masked_identity: &str,
) -> String {
    if secrets
        .into_iter()
        .flatten()
        .filter(|secret| secret.len() >= 4)
        .any(|secret| label.contains(secret))
    {
        masked_identity.to_string()
    } else {
        label
    }
}

fn redact_optional_metadata(value: &mut Option<String>, sensitive_values: &[Option<&str>]) {
    if value.as_ref().is_some_and(|value| {
        sensitive_values
            .iter()
            .flatten()
            .filter(|sensitive| sensitive.len() >= 4)
            .any(|sensitive| value.contains(sensitive))
    }) {
        *value = None;
    }
}

fn redact_file_name(value: &str) -> String {
    let (stem, extension) = value
        .rsplit_once('.')
        .filter(|(_, extension)| {
            !extension.is_empty()
                && extension.len() <= 8
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map_or((value, None), |(stem, extension)| (stem, Some(extension)));
    let lower = stem.to_ascii_lowercase();
    let sensitive = stem.contains('@')
        || lower.starts_with("sk-")
        || lower.starts_with("eyj")
        || lower.starts_with("access-")
        || lower.starts_with("refresh-")
        || lower.starts_with("token-")
        || lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("api_key")
        || stem.len() > 64;
    let stem = if stem.contains('@') {
        mask_email(stem)
    } else if sensitive {
        "import".to_string()
    } else {
        stem.to_string()
    };
    extension.map_or(stem.clone(), |extension| format!("{stem}.{extension}"))
}

fn redact_file_name_with(value: &str, sensitive_values: &[Option<&str>]) -> String {
    if sensitive_values
        .iter()
        .flatten()
        .filter(|sensitive| sensitive.len() >= 4)
        .any(|sensitive| value.contains(sensitive))
    {
        let extension = value.rsplit_once('.').and_then(|(_, extension)| {
            (!extension.is_empty()
                && extension.len() <= 8
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .then_some(extension)
        });
        return extension.map_or_else(
            || "import".to_string(),
            |extension| format!("import.{extension}"),
        );
    }
    redact_file_name(value)
}

fn mask_email(value: &str) -> String {
    let value = value.trim();
    let Some((local, domain)) = value.split_once('@') else {
        return mask_identifier(value);
    };
    let local = local.chars().next().unwrap_or('*');
    let (domain_name, suffix) = domain.rsplit_once('.').unwrap_or((domain, ""));
    let domain = domain_name.chars().next().unwrap_or('*');
    if suffix.is_empty() {
        format!("{local}***@{domain}***")
    } else {
        format!("{local}***@{domain}***.{suffix}")
    }
}

fn mask_identifier(value: &str) -> String {
    let value = value.trim();
    if value.chars().count() <= 8 {
        return "****".to_string();
    }
    let prefix = value.chars().take(4).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

fn sha256_hex(seed: &str, secret: Option<&str>, scope: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(seed.as_bytes());
    if let Some(scope) = scope {
        digest.update([0]);
        digest.update(scope.as_bytes());
    }
    if let Some(secret) = secret {
        digest.update([0]);
        digest.update(secret.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn is_oauth_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "chatgpt" | "oauth" | "openai_oauth"
    )
}

fn is_api_key_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "apikey" | "api_key"
    )
}

fn is_token_mode(value: &str) -> bool {
    is_oauth_mode(value)
        || matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "token" | "imported_token" | "apikey" | "api_key"
        )
}

fn auth_mode_name(value: ImportAuthMode) -> &'static str {
    match value {
        ImportAuthMode::OAuth => "oauth",
        ImportAuthMode::ApiKey => "api_key",
        ImportAuthMode::ImportedToken => "imported_token",
        ImportAuthMode::Unknown => "unknown",
    }
}

fn format_name(value: ImportFormat) -> &'static str {
    match value {
        ImportFormat::JsonObject => "json_object",
        ImportFormat::JsonArray => "json_array",
        ImportFormat::JsonLines => "json_lines",
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
    }
}

const ACCESS_TOKEN_FIELDS: &[&str] = &["access_token", "accessToken"];
const REFRESH_TOKEN_FIELDS: &[&str] = &["refresh_token", "refreshToken"];
const ID_TOKEN_FIELDS: &[&str] = &["id_token", "idToken"];
const API_KEY_FIELDS: &[&str] = &["OPENAI_API_KEY", "openai_api_key", "api_key", "apiKey"];
const EMAIL_FIELDS: &[&str] = &["email", "identity_email"];
const ACCOUNT_ID_FIELDS: &[&str] = &[
    "chatgpt_account_id",
    "chatgptAccountId",
    "account_id",
    "accountId",
];
const USER_ID_FIELDS: &[&str] = &["chatgpt_user_id", "chatgptUserId", "user_id", "userId"];
const ORGANIZATION_ID_FIELDS: &[&str] = &["organization_id", "organizationId", "org_id", "orgId"];
const PLAN_FIELDS: &[&str] = &[
    "chatgpt_plan_type",
    "chatgptPlanType",
    "plan_type",
    "planType",
    "plan",
];
const EXPIRES_AT_FIELDS: &[&str] = &["expires_at", "expiresAt", "expired"];
const SUBSCRIPTION_EXPIRES_AT_FIELDS: &[&str] =
    &["subscription_expires_at", "subscriptionExpiresAt"];

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

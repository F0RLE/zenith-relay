use super::normalize_account_export_description;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
#[cfg(test)]
use std::collections::HashSet;
use std::{collections::BTreeSet, fmt};

mod formats;
mod item;
mod parse;
mod sanitization;

pub const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMPORT_ITEMS: usize = 1_024;
pub const MAX_JSON_DEPTH: usize = 32;
pub(in crate::accounts::import) const MAX_RAW_TOKEN_BYTES: usize = 64 * 1024;
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

impl ImportAuthMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::AgentIdentity => "agent_identity",
            Self::ApiKey => "api_key",
            Self::ImportedToken => "imported_token",
            Self::Unknown => "unknown",
        }
    }
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
    pub account_is_fedramp: bool,
    /// Safe, non-secret labels imported from a portable account format.
    pub tags: BTreeSet<String>,
    email: Option<RedactedValue>,
    phone: Option<RedactedValue>,
    password: Option<RedactedValue>,
    totp_secret: Option<RedactedValue>,
    secrets: ImportSecretMaterial,
}

impl ParsedImportItem {
    pub fn email(&self) -> Option<&str> {
        self.email.as_ref().map(RedactedValue::expose)
    }

    pub fn phone(&self) -> Option<&str> {
        self.phone.as_ref().map(RedactedValue::expose)
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_ref().map(RedactedValue::expose)
    }

    pub fn totp_secret(&self) -> Option<&str> {
        self.totp_secret.as_ref().map(RedactedValue::expose)
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
            .field("account_is_fedramp", &self.account_is_fedramp)
            .field("tag_count", &self.tags.len())
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("phone", &self.phone.as_ref().map(|_| "[redacted]"))
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field(
                "totp_secret",
                &self.totp_secret.as_ref().map(|_| "[redacted]"),
            )
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

pub(super) struct ParsedItem {
    pub(super) preview: ImportPreviewRow,
    pub(super) item: ParsedImportItem,
}
pub use item::chatgpt_token_identity_key;
pub(in crate::accounts::import) use parse::{check_item_count, ensure_depth};
pub use parse::{combine_import_documents, parse_import};

#[cfg(test)]
mod tests;

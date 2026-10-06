mod formats;

use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    fmt,
};

pub const MAX_ACCOUNT_EXPORT_ITEMS: usize = 256;
pub const MAX_ACCOUNT_EXPORT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ACCOUNT_EXPORT_DESCRIPTION_CHARS: usize = 2_000;
const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_METADATA_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountExportFormat {
    Zenith,
    Cpa,
    Sub2api,
    Cockpit,
    #[serde(rename = "9router")]
    NineRouter,
    Codex,
    AxonHub,
    CodexManager,
}

impl AccountExportFormat {
    pub const fn all() -> [Self; 8] {
        [
            Self::Zenith,
            Self::Cpa,
            Self::Sub2api,
            Self::Cockpit,
            Self::NineRouter,
            Self::Codex,
            Self::AxonHub,
            Self::CodexManager,
        ]
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Zenith => "zenith",
            Self::Cpa => "cpa",
            Self::Sub2api => "sub2api",
            Self::Cockpit => "cockpit",
            Self::NineRouter => "9router",
            Self::Codex => "codex",
            Self::AxonHub => "axonhub",
            Self::CodexManager => "codex-manager",
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountExportRequest {
    pub account_ids: Vec<String>,
    pub format: AccountExportFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl AccountExportRequest {
    pub fn validate(&self) -> Result<()> {
        if self.account_ids.is_empty() || self.account_ids.len() > MAX_ACCOUNT_EXPORT_ITEMS {
            return Err(validation("account export selection is invalid"));
        }
        let mut seen = HashSet::new();
        if self
            .account_ids
            .iter()
            .any(|account_id| !crate::is_ascii_token(account_id, 128) || !seen.insert(account_id))
        {
            return Err(validation("account export selection is invalid"));
        }
        let description = normalize_account_export_description(self.description.as_deref())?;
        if description.is_some() && self.format != AccountExportFormat::Zenith {
            return Err(validation(
                "account export description is only supported by Zenith",
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for AccountExportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountExportRequest")
            .field("account_count", &self.account_ids.len())
            .field("format", &self.format)
            .finish()
    }
}

#[derive(Clone)]
pub struct AccountExportCredential {
    pub label: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub password: Option<String>,
    pub totp_secret: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub organization_id: Option<String>,
    pub plan_type: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub issued_at_ms: u64,
    pub subscription_active_until_ms: Option<u64>,
    pub created_at_ms: u64,
    pub priority: i32,
    pub enabled: bool,
    /// User metadata retained for imported account records.
    pub tags: BTreeSet<String>,
}

impl fmt::Debug for AccountExportCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountExportCredential")
            .field("label", &"[redacted]")
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("phone", &self.phone.as_ref().map(|_| "[redacted]"))
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field(
                "totp_secret",
                &self.totp_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "[redacted]"),
            )
            .field("expires_at_ms", &self.expires_at_ms)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("priority", &self.priority)
            .field("enabled", &self.enabled)
            .field("tag_count", &self.tags.len())
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountExportDocument {
    pub format: AccountExportFormat,
    pub account_count: usize,
    pub file_name: String,
    pub content: String,
}

impl AccountExportDocument {
    pub fn validate(&self) -> Result<()> {
        if self.account_count == 0 || self.account_count > MAX_ACCOUNT_EXPORT_ITEMS {
            return Err(validation("account export count is invalid"));
        }
        if self.file_name.is_empty()
            || self.file_name.len() > 128
            || !self
                .file_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || !self.file_name.ends_with(".json")
        {
            return Err(validation("account export filename is invalid"));
        }
        if self.content.is_empty() || self.content.len() > MAX_ACCOUNT_EXPORT_BYTES {
            return Err(validation("account export content is invalid"));
        }
        serde_json::from_str::<Value>(&self.content)
            .map_err(|_| validation("account export content is not valid JSON"))?;
        Ok(())
    }
}

impl fmt::Debug for AccountExportDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountExportDocument")
            .field("format", &self.format)
            .field("account_count", &self.account_count)
            .field("file_name", &self.file_name)
            .field("content", &"[redacted]")
            .field("content_bytes", &self.content.len())
            .finish()
    }
}

mod build;

use build::validation;
pub use build::{build_account_export, normalize_account_export_description};

#[cfg(test)]
mod tests;

use crate::{Error, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::HashSet, fmt};

pub const MAX_ACCOUNT_EXPORT_ITEMS: usize = 256;
pub const MAX_ACCOUNT_EXPORT_BYTES: usize = 4 * 1024 * 1024;
const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_METADATA_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountExportFormat {
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
    pub const fn slug(self) -> &'static str {
        match self {
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
}

impl AccountExportRequest {
    pub fn validate(&self) -> Result<()> {
        if self.account_ids.is_empty() || self.account_ids.len() > MAX_ACCOUNT_EXPORT_ITEMS {
            return Err(validation("account export selection is invalid"));
        }
        let mut seen = HashSet::new();
        if self.account_ids.iter().any(|account_id| {
            account_id.is_empty()
                || account_id.len() > 128
                || !account_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || !seen.insert(account_id)
        }) {
            return Err(validation("account export selection is invalid"));
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
}

impl fmt::Debug for AccountExportCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountExportCredential")
            .field("label", &"[redacted]")
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
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

pub fn build_account_export(
    format: AccountExportFormat,
    accounts: &[AccountExportCredential],
    exported_at_ms: u64,
) -> Result<AccountExportDocument> {
    if accounts.is_empty() || accounts.len() > MAX_ACCOUNT_EXPORT_ITEMS {
        return Err(validation("account export count is invalid"));
    }
    for account in accounts {
        validate_account(account)?;
    }
    let exported_at = timestamp(exported_at_ms)?;
    let values = accounts
        .iter()
        .map(|account| account_value(format, account, exported_at_ms, &exported_at))
        .collect::<Result<Vec<_>>>()?;
    let value = if format == AccountExportFormat::Sub2api {
        json!({
            "exported_at": exported_at,
            "proxies": [],
            "accounts": values,
            "type": "sub2api-data",
            "version": 1,
        })
    } else if values.len() == 1 {
        values.into_iter().next().expect("one export value exists")
    } else {
        Value::Array(values)
    };
    let mut content = serde_json::to_string_pretty(&value)
        .map_err(|_| validation("account export could not be encoded"))?;
    content.push('\n');
    if content.len() > MAX_ACCOUNT_EXPORT_BYTES {
        return Err(validation("account export exceeds the size limit"));
    }
    let document = AccountExportDocument {
        format,
        account_count: accounts.len(),
        file_name: format!(
            "{}-{}.json",
            if accounts.len() == 1 {
                "account"
            } else {
                "accounts"
            },
            format.slug()
        ),
        content,
    };
    document.validate()?;
    Ok(document)
}

fn account_value(
    format: AccountExportFormat,
    account: &AccountExportCredential,
    exported_at_ms: u64,
    exported_at: &str,
) -> Result<Value> {
    let expires_at = optional_timestamp(account.expires_at_ms)?;
    let subscription_expires_at = optional_timestamp(account.subscription_active_until_ms)?;
    let created_at = timestamp(if account.created_at_ms == 0 {
        account.issued_at_ms
    } else {
        account.created_at_ms
    })?;
    let issued_at = timestamp(account.issued_at_ms)?;
    let expires_in = account
        .expires_at_ms
        .and_then(|expires_at| expires_at.checked_sub(exported_at_ms))
        .map(|milliseconds| milliseconds / 1_000);
    let value = match format {
        AccountExportFormat::Cpa => json!({
            "type": "codex",
            "account_id": account.account_id,
            "chatgpt_account_id": account.account_id,
            "email": account.email,
            "name": account.label,
            "plan_type": account.plan_type,
            "chatgpt_plan_type": account.plan_type,
            "id_token": account.id_token,
            "access_token": account.access_token,
            "refresh_token": account.refresh_token.as_deref().unwrap_or(""),
            "last_refresh": exported_at,
            "expired": expires_at,
            "disabled": (!account.enabled).then_some(true),
        }),
        AccountExportFormat::Sub2api => json!({
            "name": account.label,
            "platform": "openai",
            "type": "oauth",
            "credentials": {
                "access_token": account.access_token,
                "expires_at": expires_at,
                "refresh_token": account.refresh_token,
                "id_token": account.id_token,
                "email": account.email,
                "chatgpt_account_id": account.account_id,
                "chatgpt_user_id": account.user_id,
                "organization_id": account.organization_id,
                "plan_type": account.plan_type,
                "subscription_expires_at": subscription_expires_at,
            },
            "concurrency": 0,
            "priority": account.priority,
        }),
        AccountExportFormat::Cockpit => json!({
            "type": "codex",
            "id_token": account.id_token,
            "access_token": account.access_token,
            "refresh_token": account.refresh_token.as_deref().unwrap_or(""),
            "account_id": account.account_id,
            "last_refresh": exported_at,
            "email": account.email,
            "expired": expires_at,
        }),
        AccountExportFormat::NineRouter => json!({
            "accessToken": account.access_token,
            "refreshToken": account.refresh_token,
            "expiresAt": expires_at,
            "testStatus": "active",
            "expiresIn": expires_in,
            "providerSpecificData": {
                "chatgptAccountId": account.account_id,
                "chatgptUserId": account.user_id,
                "chatgptPlanType": account.plan_type,
            },
            "id": account.account_id,
            "provider": "codex",
            "authType": "oauth",
            "name": account.label,
            "email": account.email,
            "priority": account.priority,
            "isActive": account.enabled,
            "createdAt": created_at,
            "updatedAt": exported_at,
        }),
        AccountExportFormat::Codex => {
            let mut root = object(json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "id_token": account.id_token.as_deref().unwrap_or(""),
                    "access_token": account.access_token,
                    "refresh_token": account.refresh_token.as_deref().unwrap_or(""),
                    "account_id": account.account_id.as_deref().unwrap_or(""),
                },
                "last_refresh": exported_at,
            }));
            root.insert("OPENAI_API_KEY".to_string(), Value::Null);
            Value::Object(root)
        }
        AccountExportFormat::AxonHub => json!({
            "auth_mode": "chatgpt",
            "last_refresh": issued_at,
            "tokens": {
                "access_token": account.access_token,
                "refresh_token": account.refresh_token.as_deref().unwrap_or("__missing_refresh_token__"),
                "id_token": account.id_token.as_deref().unwrap_or(""),
            },
            "axonhub_refresh_token_placeholder": account.refresh_token.is_none().then_some(true),
            "axonhub_note": account.refresh_token.is_none().then_some("refresh_token is a placeholder; access_token works only until it expires."),
        }),
        AccountExportFormat::CodexManager => json!({
            "tokens": {
                "access_token": account.access_token,
                "refresh_token": account.refresh_token.as_deref().unwrap_or(""),
                "id_token": account.id_token.as_deref().unwrap_or(""),
                "account_id": account.account_id,
                "chatgpt_account_id": account.account_id,
            },
            "meta": {
                "label": account.label,
                "chatgpt_account_id": account.account_id,
            },
        }),
    };
    Ok(if format == AccountExportFormat::Codex {
        value
    } else {
        strip_nulls(value)
    })
}

fn validate_account(account: &AccountExportCredential) -> Result<()> {
    validate_text(
        &account.label,
        "account export label",
        MAX_METADATA_BYTES,
        false,
    )?;
    validate_text(
        &account.access_token,
        "account export access token",
        MAX_SECRET_BYTES,
        false,
    )?;
    for value in [
        account.refresh_token.as_deref(),
        account.id_token.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_text(value, "account export token", MAX_SECRET_BYTES, false)?;
    }
    for value in [
        account.email.as_deref(),
        account.account_id.as_deref(),
        account.user_id.as_deref(),
        account.organization_id.as_deref(),
        account.plan_type.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_text(value, "account export metadata", MAX_METADATA_BYTES, false)?;
    }
    for value in [account.issued_at_ms, account.created_at_ms] {
        timestamp(value)?;
    }
    optional_timestamp(account.expires_at_ms)?;
    optional_timestamp(account.subscription_active_until_ms)?;
    Ok(())
}

fn validate_text(value: &str, field: &str, max: usize, allow_empty: bool) -> Result<()> {
    if (!allow_empty && value.is_empty())
        || value.len() > max
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(validation(&format!("{field} is invalid")))
    } else {
        Ok(())
    }
}

fn timestamp(milliseconds: u64) -> Result<String> {
    let milliseconds = i64::try_from(milliseconds)
        .map_err(|_| validation("account export timestamp is invalid"))?;
    DateTime::<Utc>::from_timestamp_millis(milliseconds)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or_else(|| validation("account export timestamp is invalid"))
}

fn optional_timestamp(milliseconds: Option<u64>) -> Result<Option<String>> {
    milliseconds.map(timestamp).transpose()
}

fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(strip_nulls).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .filter_map(|(key, value)| (!value.is_null()).then(|| (key, strip_nulls(value))))
                .collect(),
        ),
        value => value,
    }
}

fn object(value: Value) -> Map<String, Value> {
    match strip_nulls(value) {
        Value::Object(value) => value,
        _ => unreachable!("static account export value is an object"),
    }
}

fn validation(message: &str) -> Error {
    Error::Validation(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "synthetic-export-access-token";
    const REFRESH: &str = "synthetic-export-refresh-token";
    const ID_TOKEN: &str = "synthetic.export.id-token";

    #[test]
    fn all_supported_formats_are_valid_json_and_never_include_proxy_fields() {
        for format in formats() {
            let document = build_account_export(format, &[fixture()], 1_788_000_000_000).unwrap();
            document.validate().unwrap();
            let value: Value = serde_json::from_str(&document.content).unwrap();
            assert!(document.content.contains(ACCESS), "{format:?}");
            assert!(document.content.contains(REFRESH), "{format:?}");
            if format != AccountExportFormat::NineRouter {
                assert!(document.content.contains(ID_TOKEN), "{format:?}");
            }
            assert!(!document.content.contains("proxy.example"), "{format:?}");
            assert!(!document.content.contains("proxy_url"), "{format:?}");
            assert!(value.is_object());
        }
    }

    #[test]
    fn sub2api_matches_the_versioned_popular_export_container() {
        let document = build_account_export(
            AccountExportFormat::Sub2api,
            &[fixture(), fixture()],
            1_788_000_000_000,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&document.content).unwrap();
        assert_eq!(value["type"], "sub2api-data");
        assert_eq!(value["version"], 1);
        assert_eq!(value["proxies"], json!([]));
        assert_eq!(value["accounts"].as_array().unwrap().len(), 2);
        assert_eq!(value["accounts"][0]["credentials"]["plan_type"], "plus");
        assert_eq!(document.file_name, "accounts-sub2api.json");
    }

    #[test]
    fn single_exports_are_objects_and_bulk_exports_are_arrays() {
        for format in formats()
            .into_iter()
            .filter(|format| *format != AccountExportFormat::Sub2api)
        {
            let single = build_account_export(format, &[fixture()], 1_788_000_000_000).unwrap();
            let bulk =
                build_account_export(format, &[fixture(), fixture()], 1_788_000_000_000).unwrap();
            assert!(serde_json::from_str::<Value>(&single.content)
                .unwrap()
                .is_object());
            assert!(serde_json::from_str::<Value>(&bulk.content)
                .unwrap()
                .is_array());
        }
    }

    #[test]
    fn codex_export_preserves_the_required_null_api_key_field() {
        let document =
            build_account_export(AccountExportFormat::Codex, &[fixture()], 1_788_000_000_000)
                .unwrap();
        let value: Value = serde_json::from_str(&document.content).unwrap();
        assert!(value.get("OPENAI_API_KEY").is_some_and(Value::is_null));
    }

    #[test]
    fn debug_and_validation_do_not_expose_exported_secrets() {
        let credential = fixture();
        let document = build_account_export(
            AccountExportFormat::Codex,
            std::slice::from_ref(&credential),
            1_788_000_000_000,
        )
        .unwrap();
        let debug = format!("{credential:?} {document:?}");
        for secret in [
            ACCESS,
            REFRESH,
            ID_TOKEN,
            "person@example.test",
            "account-secret-id",
        ] {
            assert!(!debug.contains(secret));
        }

        let invalid = AccountExportDocument {
            format: AccountExportFormat::Codex,
            account_count: 1,
            file_name: "../auth.json".into(),
            content: document.content,
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn request_rejects_duplicates_and_unsafe_account_ids() {
        let valid = AccountExportRequest {
            account_ids: vec!["account_safe".into()],
            format: AccountExportFormat::Sub2api,
        };
        assert!(valid.validate().is_ok());
        for account_ids in [
            Vec::new(),
            vec!["account_safe".into(), "account_safe".into()],
            vec!["../account".into()],
        ] {
            assert!(AccountExportRequest {
                account_ids,
                format: AccountExportFormat::Sub2api,
            }
            .validate()
            .is_err());
        }
    }

    fn formats() -> [AccountExportFormat; 7] {
        [
            AccountExportFormat::Cpa,
            AccountExportFormat::Sub2api,
            AccountExportFormat::Cockpit,
            AccountExportFormat::NineRouter,
            AccountExportFormat::Codex,
            AccountExportFormat::AxonHub,
            AccountExportFormat::CodexManager,
        ]
    }

    fn fixture() -> AccountExportCredential {
        AccountExportCredential {
            label: "Synthetic Plus".into(),
            email: Some("person@example.test".into()),
            access_token: ACCESS.into(),
            refresh_token: Some(REFRESH.into()),
            id_token: Some(ID_TOKEN.into()),
            account_id: Some("account-secret-id".into()),
            user_id: Some("user-secret-id".into()),
            organization_id: Some("organization-secret-id".into()),
            plan_type: Some("plus".into()),
            expires_at_ms: Some(1_788_003_600_000),
            issued_at_ms: 1_788_000_000_000,
            subscription_active_until_ms: Some(1_900_000_000_000),
            created_at_ms: 1_787_000_000_000,
            priority: 10,
            enabled: true,
        }
    }
}

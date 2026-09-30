use super::formats;
use super::{
    AccountExportCredential, AccountExportDocument, AccountExportFormat, MAX_ACCOUNT_EXPORT_BYTES,
    MAX_ACCOUNT_EXPORT_DESCRIPTION_CHARS, MAX_ACCOUNT_EXPORT_ITEMS, MAX_METADATA_BYTES,
    MAX_SECRET_BYTES,
};
use crate::{Error, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};

pub fn build_account_export(
    format: AccountExportFormat,
    accounts: &[AccountExportCredential],
    exported_at_ms: u64,
    description: Option<&str>,
) -> Result<AccountExportDocument> {
    if accounts.is_empty() || accounts.len() > MAX_ACCOUNT_EXPORT_ITEMS {
        return Err(validation("account export count is invalid"));
    }
    for account in accounts {
        validate_account(account)?;
    }
    let description = normalize_account_export_description(description)?;
    if description.is_some() && format != AccountExportFormat::Zenith {
        return Err(validation(
            "account export description is only supported by Zenith",
        ));
    }
    let exported_at_value = timestamp_value(exported_at_ms)?;
    let exported_at = exported_at_value.to_rfc3339_opts(SecondsFormat::Millis, true);
    let values = accounts
        .iter()
        .map(|account| formats::account_value(format, account, exported_at_ms, &exported_at))
        .collect::<Result<Vec<_>>>()?;
    let value = if format == AccountExportFormat::Zenith {
        strip_nulls(json!({
            "format": "zenith",
            "version": 1,
            "exportedAt": exported_at,
            "description": description,
            "accounts": values,
        }))
    } else if format == AccountExportFormat::Sub2api {
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
        file_name: if format == AccountExportFormat::Zenith {
            "zenith.json".into()
        } else {
            format!(
                "{}-{}.json",
                if accounts.len() == 1 {
                    "account"
                } else {
                    "accounts"
                },
                format.slug()
            )
        },
        content,
    };
    document.validate()?;
    Ok(document)
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

pub(super) fn timestamp(milliseconds: u64) -> Result<String> {
    Ok(timestamp_value(milliseconds)?.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn timestamp_value(milliseconds: u64) -> Result<DateTime<Utc>> {
    let milliseconds = i64::try_from(milliseconds)
        .map_err(|_| validation("account export timestamp is invalid"))?;
    DateTime::<Utc>::from_timestamp_millis(milliseconds)
        .ok_or_else(|| validation("account export timestamp is invalid"))
}

pub fn normalize_account_export_description(value: Option<&str>) -> Result<Option<&str>> {
    let value = crate::omit_blank(value);
    if value.is_some_and(|value| {
        value.chars().count() > MAX_ACCOUNT_EXPORT_DESCRIPTION_CHARS
            || value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    }) {
        return Err(validation("account export description is invalid"));
    }
    Ok(value)
}

pub(super) fn optional_timestamp(milliseconds: Option<u64>) -> Result<Option<String>> {
    milliseconds.map(timestamp).transpose()
}

pub(super) fn strip_nulls(value: Value) -> Value {
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

pub(super) fn object(value: Value) -> Map<String, Value> {
    match strip_nulls(value) {
        Value::Object(value) => value,
        _ => unreachable!("static account export value is an object"),
    }
}

pub(super) fn validation(message: &str) -> Error {
    Error::Validation(message.to_string())
}

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
    let account_exports = accounts
        .iter()
        .map(|account| formats::build_account_export(format, account, exported_at_ms, &exported_at))
        .collect::<Result<Vec<_>>>()?;
    let export_payload = if format == AccountExportFormat::Zenith {
        strip_nulls(json!({
            "format": "zenith",
            "version": 1,
            "exportedAt": exported_at,
            "description": description,
            "accounts": account_exports,
        }))
    } else if format == AccountExportFormat::Sub2api {
        json!({
            "exported_at": exported_at,
            "proxies": [],
            "accounts": account_exports,
            "type": "sub2api-data",
            "version": 1,
        })
    } else if account_exports.len() == 1 {
        account_exports
            .into_iter()
            .next()
            .expect("one account export exists")
    } else {
        Value::Array(account_exports)
    };
    let mut content = serde_json::to_string_pretty(&export_payload)
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
    for token_text in [
        account.refresh_token.as_deref(),
        account.id_token.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_text(token_text, "account export token", MAX_SECRET_BYTES, false)?;
    }
    for metadata_text in [
        account.email.as_deref(),
        account.account_id.as_deref(),
        account.user_id.as_deref(),
        account.organization_id.as_deref(),
        account.plan_type.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_text(
            metadata_text,
            "account export metadata",
            MAX_METADATA_BYTES,
            false,
        )?;
    }
    for timestamp_ms in [account.issued_at_ms, account.created_at_ms] {
        timestamp(timestamp_ms)?;
    }
    optional_timestamp(account.expires_at_ms)?;
    optional_timestamp(account.subscription_active_until_ms)?;
    Ok(())
}

fn validate_text(
    text_value: &str,
    field_name: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<()> {
    if (!allow_empty && text_value.is_empty())
        || text_value.len() > max_bytes
        || text_value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(validation(&format!("{field_name} is invalid")))
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

pub fn normalize_account_export_description(description: Option<&str>) -> Result<Option<&str>> {
    let trimmed_description = crate::omit_blank(description);
    if trimmed_description.is_some_and(|description_text| {
        description_text.chars().count() > MAX_ACCOUNT_EXPORT_DESCRIPTION_CHARS
            || description_text
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    }) {
        return Err(validation("account export description is invalid"));
    }
    Ok(trimmed_description)
}

pub(super) fn optional_timestamp(milliseconds: Option<u64>) -> Result<Option<String>> {
    milliseconds.map(timestamp).transpose()
}

pub(super) fn strip_nulls(json_value: Value) -> Value {
    match json_value {
        Value::Array(array_values) => {
            Value::Array(array_values.into_iter().map(strip_nulls).collect())
        }
        Value::Object(object_values) => Value::Object(
            object_values
                .into_iter()
                .filter_map(|(key, field_value)| {
                    (!field_value.is_null()).then(|| (key, strip_nulls(field_value)))
                })
                .collect(),
        ),
        other_value => other_value,
    }
}

pub(super) fn object(json_value: Value) -> Map<String, Value> {
    match strip_nulls(json_value) {
        Value::Object(object_values) => object_values,
        _ => unreachable!("static account export value is an object"),
    }
}

pub(super) fn validation(message: &str) -> Error {
    Error::Validation(message.to_string())
}

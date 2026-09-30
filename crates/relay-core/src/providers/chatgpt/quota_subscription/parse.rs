use super::CodexSubscriptionMetadata;
use crate::accounts::decode_unverified_jwt_payload;
use crate::error_codes;
use crate::quota::QuotaRefreshFailure;
use chrono::{TimeZone, Utc};
use serde_json::{Map, Value};
pub(super) fn parse_accounts_check(
    payload: &Value,
    preferred_account_id: &str,
) -> Result<CodexSubscriptionMetadata, QuotaRefreshFailure> {
    let records = account_records(payload);
    if records.is_empty() {
        return Err(super::failure(
            error_codes::SUBSCRIPTION_ACCOUNT_MISSING,
            false,
        ));
    }
    let ordered_key = payload
        .get("account_ordering")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let selected = records
        .iter()
        .find(|record| record_account_id(record).as_deref() == Some(preferred_account_id))
        .or_else(|| {
            ordered_key.and_then(|ordered_key| {
                records.iter().find(|record| {
                    record.key.as_deref() == Some(ordered_key)
                        || record_account_id(record).as_deref() == Some(ordered_key)
                })
            })
        })
        .unwrap_or(&records[0]);
    let record = selected
        .node
        .as_object()
        .ok_or_else(|| super::failure(error_codes::SUBSCRIPTION_INVALID_RESPONSE, false))?;
    let account = record
        .get("account")
        .and_then(Value::as_object)
        .unwrap_or(record);
    let entitlement = record
        .get("entitlement")
        .and_then(Value::as_object)
        .or_else(|| account.get("entitlement").and_then(Value::as_object));
    Ok(CodexSubscriptionMetadata {
        account_id: record_account_id(selected),
        plan_type: entitlement
            .and_then(|value| string_field(value, &["subscription_plan", "plan_type"]))
            .or_else(|| string_field(account, &["plan_type", "planType"])),
        active_until_ms: entitlement
            .and_then(|value| timestamp_field(value, &["expires_at", "active_until"]))
            .or_else(|| {
                timestamp_field(
                    account,
                    &["expires_at", "active_until", "subscription_active_until"],
                )
            }),
    })
}

pub(super) fn parse_subscriptions(payload: &Value, account_id: &str) -> CodexSubscriptionMetadata {
    let candidates = [
        Some(payload),
        payload.get("data"),
        payload.get("subscription"),
        payload
            .get("data")
            .and_then(|value| value.get("subscription")),
    ];
    let mut metadata = CodexSubscriptionMetadata {
        account_id: Some(account_id.to_string()),
        ..Default::default()
    };
    for object in candidates
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        metadata.plan_type = metadata
            .plan_type
            .or_else(|| string_field(object, &["subscription_plan", "plan_type", "planType"]));
        metadata.active_until_ms = metadata.active_until_ms.or_else(|| {
            timestamp_field(
                object,
                &[
                    "active_until",
                    "activeUntil",
                    "expires_at",
                    "expiresAt",
                    "subscription_active_until",
                ],
            )
        });
    }
    metadata
}

struct AccountCheckRecord {
    key: Option<String>,
    node: Value,
}

/// The result of an authenticated ChatGPT account-check response is the only
/// authoritative source for an imported account identity. IDs found in an
/// import document or an unsigned JWT payload are hints only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountCheckIdentityError {
    /// The authenticated response did not contain a usable account id.
    Missing,
    /// The authenticated response contained account ids, but none matched the
    /// identity hint supplied by the import.
    Mismatch,
}

/// Returns all bounded account ids in an authenticated account-check payload,
/// preserving the provider's account ordering where it supplies one.
pub fn account_ids_from_check_response(payload: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for record in account_records(payload) {
        let Some(account_id) = record_account_id(&record) else {
            continue;
        };
        if !ids
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&account_id))
        {
            ids.push(account_id);
        }
    }
    ids
}

/// Selects the authenticated account which matches the imported identity
/// hints. With no hints, the provider's first ordered account is used.
pub fn resolve_account_check_account_id(
    payload: &Value,
    claimed_account_ids: &[&str],
) -> Result<String, AccountCheckIdentityError> {
    let account_ids = account_ids_from_check_response(payload);
    if account_ids.is_empty() {
        return Err(AccountCheckIdentityError::Missing);
    }
    if claimed_account_ids.is_empty() {
        return Ok(account_ids[0].clone());
    }
    account_ids
        .iter()
        .find(|account_id| {
            claimed_account_ids
                .iter()
                .map(|value| value.trim())
                .any(|value| !value.is_empty() && value.eq_ignore_ascii_case(account_id))
        })
        .cloned()
        .ok_or(AccountCheckIdentityError::Mismatch)
}

/// Extracts account ids from a JWT without treating the unsigned claims as
/// trusted. Callers must still reconcile these hints with an authenticated
/// account-check response before persisting them.
pub fn unverified_chatgpt_account_id_hints(token: &str) -> Vec<String> {
    let Some(payload) = decode_unverified_jwt_payload::<Value>(token) else {
        return Vec::new();
    };
    let Some(auth) = payload
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for name in ["chatgpt_account_id", "account_id"] {
        let Some(value) = auth.get(name).and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if value.is_empty()
            || value.len() > super::MAX_ACCOUNT_ID_BYTES
            || value.chars().any(char::is_control)
            || ids.iter().any(|existing: &String| existing == value)
        {
            continue;
        }
        ids.push(value.to_string());
    }
    ids
}

fn account_records(payload: &Value) -> Vec<AccountCheckRecord> {
    let Some(accounts) = payload.get("accounts") else {
        return (payload.is_object()
            && record_account_id(&AccountCheckRecord {
                key: None,
                node: payload.clone(),
            })
            .is_some())
        .then(|| AccountCheckRecord {
            key: None,
            node: payload.clone(),
        })
        .into_iter()
        .collect();
    };
    match accounts {
        Value::Array(values) => values
            .iter()
            .filter(|value| value.is_object())
            .map(|node| AccountCheckRecord {
                key: None,
                node: node.clone(),
            })
            .collect(),
        Value::Object(values) => {
            let mut records = Vec::with_capacity(values.len());
            let ordered_keys = payload
                .get("account_ordering")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            for key in ordered_keys {
                if let Some(node) = values.get(key).filter(|value| value.is_object()) {
                    records.push(AccountCheckRecord {
                        key: Some(key.to_string()),
                        node: node.clone(),
                    });
                }
            }
            for (key, node) in values.iter().filter(|(_, value)| value.is_object()) {
                if !records
                    .iter()
                    .any(|record| record.key.as_deref() == Some(key.as_str()))
                {
                    records.push(AccountCheckRecord {
                        key: Some(key.clone()),
                        node: node.clone(),
                    });
                }
            }
            records
        }
        _ => Vec::new(),
    }
}

fn record_account_id(record: &AccountCheckRecord) -> Option<String> {
    let object = record.node.as_object()?;
    let account = object
        .get("account")
        .and_then(Value::as_object)
        .unwrap_or(object);
    string_field(
        account,
        &["account_id", "id", "chatgpt_account_id", "workspace_id"],
    )
    .or_else(|| record.key.clone())
}

fn string_field(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        })
        .map(str::to_string)
}

fn timestamp_field(object: &Map<String, Value>, names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(parse_subscription_timestamp_ms))
}

pub fn parse_subscription_timestamp_ms(value: &Value) -> Option<u64> {
    match value {
        Value::Number(value) => value
            .as_u64()
            .or_else(|| {
                let value = value.as_f64()?;
                (value.is_finite() && value >= 0.0 && value <= u64::MAX as f64)
                    .then(|| value.trunc() as u64)
            })
            .and_then(normalize_epoch_ms),
        Value::String(value) => parse_subscription_timestamp_text(value),
        _ => None,
    }
}

pub fn parse_subscription_timestamp_text(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    value
        .parse::<u64>()
        .ok()
        .and_then(normalize_epoch_ms)
        .or_else(|| crate::unix_time_ms_from_rfc3339(value))
}

fn normalize_epoch_ms(value: u64) -> Option<u64> {
    let value = if value < 100_000_000_000 {
        value.checked_mul(1_000)?
    } else {
        value
    };
    let signed = i64::try_from(value).ok()?;
    Utc.timestamp_millis_opt(signed).single()?;
    Some(value)
}

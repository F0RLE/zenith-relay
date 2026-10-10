use super::CodexSubscriptionMetadata;
use crate::accounts::decode_unverified_jwt_payload;
use crate::error_codes;
use crate::quota::QuotaRefreshFailure;
use chrono::{TimeZone, Utc};
use serde_json::{Map, Value};
pub(super) fn parse_accounts_check(
    account_check_response: &Value,
    preferred_account_id: &str,
) -> Result<CodexSubscriptionMetadata, QuotaRefreshFailure> {
    let account_check_records = account_records(account_check_response);
    if account_check_records.is_empty() {
        return Err(super::failure(
            error_codes::SUBSCRIPTION_ACCOUNT_MISSING,
            false,
        ));
    }
    let ordered_key = account_check_response
        .get("account_ordering")
        .and_then(Value::as_array)
        .and_then(|ordering_entries| ordering_entries.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|ordered_account_id| !ordered_account_id.is_empty());
    let selected = account_check_records
        .iter()
        .find(|account_record| {
            record_account_id(account_record).as_deref() == Some(preferred_account_id)
        })
        .or_else(|| {
            ordered_key.and_then(|ordered_key| {
                account_check_records.iter().find(|account_record| {
                    account_record.map_key.as_deref() == Some(ordered_key)
                        || record_account_id(account_record).as_deref() == Some(ordered_key)
                })
            })
        })
        .unwrap_or(&account_check_records[0]);
    let selected_account_object = selected
        .account_value
        .as_object()
        .ok_or_else(|| super::failure(error_codes::SUBSCRIPTION_INVALID_RESPONSE, false))?;
    let account_fields = selected_account_object
        .get("account")
        .and_then(Value::as_object)
        .unwrap_or(selected_account_object);
    let entitlement = selected_account_object
        .get("entitlement")
        .and_then(Value::as_object)
        .or_else(|| account_fields.get("entitlement").and_then(Value::as_object));
    Ok(CodexSubscriptionMetadata {
        account_id: record_account_id(selected),
        plan_type: entitlement
            .and_then(|entitlement_fields| {
                string_field(entitlement_fields, &["subscription_plan", "plan_type"])
            })
            .or_else(|| string_field(account_fields, &["plan_type", "planType"])),
        active_until_ms: entitlement
            .and_then(|entitlement_fields| {
                timestamp_field(entitlement_fields, &["expires_at", "active_until"])
            })
            .or_else(|| {
                timestamp_field(
                    account_fields,
                    &["expires_at", "active_until", "subscription_active_until"],
                )
            }),
    })
}

pub(super) fn parse_subscriptions(
    subscription_response: &Value,
    account_id: &str,
) -> CodexSubscriptionMetadata {
    let candidates = [
        Some(subscription_response),
        subscription_response.get("data"),
        subscription_response.get("subscription"),
        subscription_response
            .get("data")
            .and_then(|data_object| data_object.get("subscription")),
    ];
    let mut metadata = CodexSubscriptionMetadata {
        account_id: Some(account_id.to_string()),
        ..Default::default()
    };
    for subscription_object in candidates
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        metadata.plan_type = metadata.plan_type.or_else(|| {
            string_field(
                subscription_object,
                &["subscription_plan", "plan_type", "planType"],
            )
        });
        metadata.active_until_ms = metadata.active_until_ms.or_else(|| {
            timestamp_field(
                subscription_object,
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
    map_key: Option<String>,
    account_value: Value,
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
pub fn account_ids_from_check_response(account_check_response: &Value) -> Vec<String> {
    let mut account_ids = Vec::new();
    for account_record in account_records(account_check_response) {
        let Some(account_id) = record_account_id(&account_record) else {
            continue;
        };
        if !account_ids
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&account_id))
        {
            account_ids.push(account_id);
        }
    }
    account_ids
}

/// Selects the authenticated account which matches the imported identity
/// hints. With no hints, the provider's first ordered account is used.
pub fn resolve_account_check_account_id(
    account_check_response: &Value,
    claimed_account_ids: &[&str],
) -> Result<String, AccountCheckIdentityError> {
    let account_ids = account_ids_from_check_response(account_check_response);
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
                .map(|claimed_account_id| claimed_account_id.trim())
                .any(|claimed_account_id| {
                    !claimed_account_id.is_empty()
                        && claimed_account_id.eq_ignore_ascii_case(account_id)
                })
        })
        .cloned()
        .ok_or(AccountCheckIdentityError::Mismatch)
}

/// Extracts account ids from a JWT without treating the unsigned claims as
/// trusted. Callers must still reconcile these hints with an authenticated
/// account-check response before persisting them.
pub fn unverified_chatgpt_account_id_hints(token: &str) -> Vec<String> {
    let Some(token_claims) = decode_unverified_jwt_payload::<Value>(token) else {
        return Vec::new();
    };
    let Some(auth) = token_claims
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut account_ids = Vec::new();
    for field_name in ["chatgpt_account_id", "account_id"] {
        let Some(account_id_hint) = auth.get(field_name).and_then(Value::as_str).map(str::trim)
        else {
            continue;
        };
        if account_id_hint.is_empty()
            || account_id_hint.len() > super::MAX_ACCOUNT_ID_BYTES
            || account_id_hint.chars().any(char::is_control)
            || account_ids
                .iter()
                .any(|existing_account_id: &String| existing_account_id == account_id_hint)
        {
            continue;
        }
        account_ids.push(account_id_hint.to_string());
    }
    account_ids
}

fn account_records(account_check_response: &Value) -> Vec<AccountCheckRecord> {
    let Some(accounts) = account_check_response.get("accounts") else {
        return (account_check_response.is_object()
            && record_account_id(&AccountCheckRecord {
                map_key: None,
                account_value: account_check_response.clone(),
            })
            .is_some())
        .then(|| AccountCheckRecord {
            map_key: None,
            account_value: account_check_response.clone(),
        })
        .into_iter()
        .collect();
    };
    match accounts {
        Value::Array(account_values) => account_values
            .iter()
            .filter(|account_value| account_value.is_object())
            .map(|account_value| AccountCheckRecord {
                map_key: None,
                account_value: account_value.clone(),
            })
            .collect(),
        Value::Object(account_map) => {
            let mut account_records = Vec::with_capacity(account_map.len());
            let ordered_keys = account_check_response
                .get("account_ordering")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            for map_key in ordered_keys {
                if let Some(account_value) = account_map
                    .get(map_key)
                    .filter(|account_value| account_value.is_object())
                {
                    account_records.push(AccountCheckRecord {
                        map_key: Some(map_key.to_string()),
                        account_value: account_value.clone(),
                    });
                }
            }
            for (map_key, account_value) in account_map
                .iter()
                .filter(|(_, account_value)| account_value.is_object())
            {
                if !account_records.iter().any(|account_record| {
                    account_record.map_key.as_deref() == Some(map_key.as_str())
                }) {
                    account_records.push(AccountCheckRecord {
                        map_key: Some(map_key.clone()),
                        account_value: account_value.clone(),
                    });
                }
            }
            account_records
        }
        _ => Vec::new(),
    }
}

fn record_account_id(account_record: &AccountCheckRecord) -> Option<String> {
    let account_value = account_record.account_value.as_object()?;
    let account_object = account_value
        .get("account")
        .and_then(Value::as_object)
        .unwrap_or(account_value);
    string_field(
        account_object,
        &["account_id", "id", "chatgpt_account_id", "workspace_id"],
    )
    .or_else(|| account_record.map_key.clone())
}

fn string_field(record_fields: &Map<String, Value>, field_names: &[&str]) -> Option<String> {
    field_names
        .iter()
        .find_map(|field_name| record_fields.get(*field_name).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text_value| {
            !text_value.is_empty()
                && text_value.len() <= 128
                && !text_value.chars().any(char::is_control)
        })
        .map(str::to_string)
}

fn timestamp_field(record_fields: &Map<String, Value>, field_names: &[&str]) -> Option<u64> {
    field_names.iter().find_map(|field_name| {
        record_fields
            .get(*field_name)
            .and_then(parse_subscription_timestamp_ms)
    })
}

pub fn parse_subscription_timestamp_ms(timestamp_value: &Value) -> Option<u64> {
    match timestamp_value {
        Value::Number(number_value) => number_value
            .as_u64()
            .or_else(|| {
                let number_value = number_value.as_f64()?;
                (number_value.is_finite() && number_value >= 0.0 && number_value <= u64::MAX as f64)
                    .then(|| number_value.trunc() as u64)
            })
            .and_then(normalize_epoch_ms),
        Value::String(timestamp_text) => parse_subscription_timestamp_text(timestamp_text),
        _ => None,
    }
}

pub fn parse_subscription_timestamp_text(timestamp_text: &str) -> Option<u64> {
    let timestamp_text = timestamp_text.trim();
    if timestamp_text.is_empty() || timestamp_text.len() > 64 {
        return None;
    }
    timestamp_text
        .parse::<u64>()
        .ok()
        .and_then(normalize_epoch_ms)
        .or_else(|| crate::unix_time_ms_from_rfc3339(timestamp_text))
}

fn normalize_epoch_ms(epoch_value: u64) -> Option<u64> {
    let normalized_epoch = if epoch_value < 100_000_000_000 {
        epoch_value.checked_mul(1_000)?
    } else {
        epoch_value
    };
    let signed = i64::try_from(normalized_epoch).ok()?;
    Utc.timestamp_millis_opt(signed).single()?;
    Some(normalized_epoch)
}

use super::{ResetCredit, ResetCreditsSnapshot};
use serde_json::Value;

pub(in crate::local_pool::accounts::reset_credits) fn parse_snapshot(
    payload: &Value,
) -> ResetCreditsSnapshot {
    let mut explicit_count = None;
    let mut raw_credits = Vec::new();
    let mut credit_list_present = false;
    collect_reset_credit_values(
        payload,
        &mut explicit_count,
        &mut raw_credits,
        &mut credit_list_present,
    );
    let credits = raw_credits
        .iter()
        .filter_map(parse_credit)
        .filter(is_codex_credit)
        .collect::<Vec<_>>();
    let available_count = explicit_count.or_else(|| {
        credit_list_present.then(|| {
            credits
                .iter()
                .filter(|credit| is_credit_available(credit))
                .count() as u32
        })
    });
    let next_expires_at = credits
        .iter()
        .filter(|credit| is_credit_available(credit))
        .filter_map(|credit| credit.expires_at)
        .min();
    ResetCreditsSnapshot {
        available_count,
        credits,
        next_expires_at,
    }
}

pub(super) fn is_codex_credit(credit: &ResetCredit) -> bool {
    credit
        .reset_type
        .as_deref()
        .is_none_or(|value| value.eq_ignore_ascii_case("codex_rate_limits"))
}

fn collect_reset_credit_values(
    value: &Value,
    explicit_count: &mut Option<u32>,
    raw_credits: &mut Vec<Value>,
    credit_list_present: &mut bool,
) {
    match value {
        Value::Array(items) => {
            *credit_list_present = true;
            raw_credits.extend(items.iter().filter(|item| item.is_object()).cloned());
        }
        Value::Object(object) => {
            if explicit_count.is_none() {
                *explicit_count = ["available_count", "availableCount"]
                    .iter()
                    .find_map(|key| object.get(*key).and_then(parse_u32));
            }
            let mut nested = false;
            for key in ["credits", "rate_limit_reset_credits", "items", "data"] {
                if let Some(child) = object.get(key) {
                    nested = true;
                    collect_reset_credit_values(
                        child,
                        explicit_count,
                        raw_credits,
                        credit_list_present,
                    );
                }
            }
            if !nested && looks_like_reset_credit(object) {
                *credit_list_present = true;
                raw_credits.push(value.clone());
            }
        }
        _ => {}
    }
}

fn looks_like_reset_credit(object: &serde_json::Map<String, Value>) -> bool {
    [
        "id",
        "status",
        "state",
        "type",
        "reset_type",
        "resetType",
        "expires_at",
        "expire_at",
        "expiresAt",
        "granted_at",
        "created_at",
        "redeemed_at",
        "used_at",
        "consumed_at",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
}

fn parse_credit(value: &Value) -> Option<ResetCredit> {
    let object = value.as_object()?;
    let raw_status = string_field(object, &["status", "state"]);
    Some(ResetCredit {
        status: normalized_status(raw_status.as_deref()),
        reset_type: string_field(object, &["type", "reset_type", "resetType"]),
        granted_at: timestamp_field(object, &["granted_at", "created_at", "grantedAt"]),
        expires_at: timestamp_field(object, &["expires_at", "expire_at", "expiresAt"]),
        redeemed_at: timestamp_field(
            object,
            &["redeemed_at", "used_at", "consumed_at", "redeemedAt"],
        ),
    })
}

fn string_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object.get(*key).and_then(|value| match value {
            Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        })
    })
}

fn timestamp_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i64> {
    let value = keys.iter().find_map(|key| object.get(*key))?;
    if let Some(number) = value.as_i64() {
        return Some(normalize_timestamp(number));
    }
    let text = value.as_str()?.trim();
    if let Ok(number) = text.parse::<i64>() {
        return Some(normalize_timestamp(number));
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|date| date.timestamp())
}

fn parse_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| value.as_i64().and_then(|value| u32::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<u32>().ok())
}

fn normalize_timestamp(value: i64) -> i64 {
    if value > 1_000_000_000_000 {
        value / 1_000
    } else {
        value
    }
}

fn normalized_status(raw_status: Option<&str>) -> Option<String> {
    raw_status.map(|status| status.trim().to_ascii_lowercase())
}

fn is_credit_available(credit: &ResetCredit) -> bool {
    if credit
        .reset_type
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case("codex_rate_limits"))
    {
        return false;
    }
    let status = credit.status.as_deref().unwrap_or("available");
    if !status.is_empty() && status != "available" {
        return false;
    }
    credit
        .expires_at
        .map(|expires_at| expires_at > chrono::Utc::now().timestamp())
        .unwrap_or(true)
}

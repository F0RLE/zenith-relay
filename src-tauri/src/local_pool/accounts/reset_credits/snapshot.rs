use super::{ResetCredit, ResetCreditsSnapshot};
use serde_json::Value;

pub(in crate::local_pool::accounts::reset_credits) fn parse_snapshot(
    reset_credits_response: &Value,
) -> ResetCreditsSnapshot {
    let mut explicit_count = None;
    let mut raw_credits = Vec::new();
    let mut credit_list_present = false;
    collect_reset_credit_values(
        reset_credits_response,
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
        .is_none_or(|reset_type| reset_type.eq_ignore_ascii_case("codex_rate_limits"))
}

fn collect_reset_credit_values(
    snapshot_value: &Value,
    explicit_count: &mut Option<u32>,
    raw_credits: &mut Vec<Value>,
    credit_list_present: &mut bool,
) {
    match snapshot_value {
        Value::Array(reset_credit_entries) => {
            *credit_list_present = true;
            raw_credits.extend(
                reset_credit_entries
                    .iter()
                    .filter(|credit_entry| credit_entry.is_object())
                    .cloned(),
            );
        }
        Value::Object(snapshot_object) => {
            if explicit_count.is_none() {
                *explicit_count = ["available_count", "availableCount"]
                    .iter()
                    .find_map(|key| snapshot_object.get(*key).and_then(parse_u32));
            }
            let mut nested = false;
            for key in ["credits", "rate_limit_reset_credits", "items", "data"] {
                if let Some(child) = snapshot_object.get(key) {
                    nested = true;
                    collect_reset_credit_values(
                        child,
                        explicit_count,
                        raw_credits,
                        credit_list_present,
                    );
                }
            }
            if !nested && looks_like_reset_credit(snapshot_object) {
                *credit_list_present = true;
                raw_credits.push(snapshot_value.clone());
            }
        }
        _ => {}
    }
}

fn looks_like_reset_credit(credit_object: &serde_json::Map<String, Value>) -> bool {
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
    .any(|key| credit_object.contains_key(*key))
}

fn parse_credit(credit_json: &Value) -> Option<ResetCredit> {
    let credit_object = credit_json.as_object()?;
    let raw_status = string_field(credit_object, &["status", "state"]);
    Some(ResetCredit {
        status: normalized_status(raw_status.as_deref()),
        reset_type: string_field(credit_object, &["type", "reset_type", "resetType"]),
        granted_at: timestamp_field(credit_object, &["granted_at", "created_at", "grantedAt"]),
        expires_at: timestamp_field(credit_object, &["expires_at", "expire_at", "expiresAt"]),
        redeemed_at: timestamp_field(
            credit_object,
            &["redeemed_at", "used_at", "consumed_at", "redeemedAt"],
        ),
    })
}

fn string_field(credit_object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        credit_object
            .get(*key)
            .and_then(|field_value| match field_value {
                Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
                Value::Number(number) => Some(number.to_string()),
                Value::Bool(flag) => Some(flag.to_string()),
                _ => None,
            })
    })
}

fn timestamp_field(credit_object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i64> {
    let timestamp_json = keys.iter().find_map(|key| credit_object.get(*key))?;
    if let Some(number) = timestamp_json.as_i64() {
        return Some(normalize_timestamp(number));
    }
    let text = timestamp_json.as_str()?.trim();
    if let Ok(number) = text.parse::<i64>() {
        return Some(normalize_timestamp(number));
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|date| date.timestamp())
}

fn parse_u32(count_value: &Value) -> Option<u32> {
    count_value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .or_else(|| {
            count_value
                .as_i64()
                .and_then(|number| u32::try_from(number).ok())
        })
        .or_else(|| count_value.as_str()?.trim().parse::<u32>().ok())
}

fn normalize_timestamp(timestamp_value: i64) -> i64 {
    if timestamp_value > 1_000_000_000_000 {
        timestamp_value / 1_000
    } else {
        timestamp_value
    }
}

fn normalized_status(raw_status: Option<&str>) -> Option<String> {
    raw_status.map(|status| status.trim().to_ascii_lowercase())
}

fn is_credit_available(credit: &ResetCredit) -> bool {
    if credit
        .reset_type
        .as_deref()
        .is_some_and(|reset_type| !reset_type.eq_ignore_ascii_case("codex_rate_limits"))
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

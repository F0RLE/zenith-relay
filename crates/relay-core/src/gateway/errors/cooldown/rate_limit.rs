use super::super::{upstream_error_text, MAX_RATE_LIMIT_RETRY_HINT_MS};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RateLimitBodyHint {
    pub(crate) retry_after_ms: Option<u64>,
    pub(crate) global: bool,
}

pub(crate) fn rate_limit_body_hint(body: &[u8]) -> RateLimitBodyHint {
    rate_limit_body_hint_at(body, SystemTime::now())
}

pub(crate) fn rate_limit_body_hint_at(body: &[u8], now: SystemTime) -> RateLimitBodyHint {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return RateLimitBodyHint::default();
    };
    rate_limit_body_hint_value(&value, now)
}

pub(crate) fn rate_limit_body_hint_value(value: &Value, now: SystemTime) -> RateLimitBodyHint {
    let retry_after_ms = rate_limit_reset_delay_ms(value, now)
        .or_else(|| {
            [
                "/resets_in_seconds",
                "/error/resets_in_seconds",
                "/body/error/resets_in_seconds",
                "/response/error/resets_in_seconds",
            ]
            .into_iter()
            .find_map(|path| value.pointer(path).and_then(json_seconds_to_ms))
        })
        .or_else(|| {
            [
                "/retry_after",
                "/error/retry_after",
                "/body/error/retry_after",
                "/response/error/retry_after",
            ]
            .into_iter()
            .find_map(|path| value.pointer(path).and_then(json_seconds_to_ms))
        })
        .or_else(|| retry_delay_from_text(&upstream_error_text(value)));
    let global = [
        "/type",
        "/code",
        "/error/type",
        "/error/code",
        "/body/error/type",
        "/body/error/code",
        "/response/error/type",
        "/response/error/code",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .map(str::to_ascii_lowercase)
    .any(|kind| {
        kind.contains("usage_limit")
            || kind.contains("usage_not_included")
            || kind.contains("quota")
            || kind.contains("credits_depleted")
            || matches!(
                kind.as_str(),
                "rate_limit_reached" | "websocket_connection_limit_reached"
            )
    });
    RateLimitBodyHint {
        retry_after_ms,
        global,
    }
}

fn rate_limit_reset_delay_ms(value: &Value, now: SystemTime) -> Option<u64> {
    let reset_at = [
        "/resets_at",
        "/error/resets_at",
        "/body/error/resets_at",
        "/response/error/resets_at",
    ]
    .into_iter()
    .find_map(|path| value.pointer(path).and_then(json_u64))?;
    let reset_seconds = if reset_at > 10_000_000_000 {
        reset_at / 1_000
    } else {
        reset_at
    };
    let now_seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    reset_seconds
        .checked_sub(now_seconds)
        .and_then(|seconds| seconds.checked_mul(1_000))
        .filter(|duration_ms| *duration_ms > 0)
        .map(|duration_ms| duration_ms.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

fn json_seconds_to_ms(value: &Value) -> Option<u64> {
    let seconds = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Some(
        (seconds * 1_000.0)
            .ceil()
            .min(MAX_RATE_LIMIT_RETRY_HINT_MS as f64) as u64,
    )
}

fn retry_delay_from_text(text: &str) -> Option<u64> {
    let suffix = text.split_once("try again in")?.1.trim_start();
    let number_end = suffix
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(suffix.len());
    let seconds_or_millis = suffix[..number_end].parse::<f64>().ok()?;
    if !seconds_or_millis.is_finite() || seconds_or_millis <= 0.0 {
        return None;
    }
    let unit = suffix[number_end..].trim_start();
    let multiplier = if unit.starts_with("ms") || unit.starts_with("millisecond") {
        1.0
    } else if unit.starts_with('s') || unit.starts_with("second") {
        1_000.0
    } else {
        return None;
    };
    Some(
        (seconds_or_millis * multiplier)
            .ceil()
            .min(MAX_RATE_LIMIT_RETRY_HINT_MS as f64) as u64,
    )
}

pub(crate) fn retry_delay_ms(header: Option<u64>, body: Option<u64>, fallback: u64) -> u64 {
    header.into_iter().chain(body).max().unwrap_or(fallback)
}

pub(crate) fn retry_after_ms(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<u64> {
    crate::transport::retry_after_ms(headers, now)
        .map(|delay| delay.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

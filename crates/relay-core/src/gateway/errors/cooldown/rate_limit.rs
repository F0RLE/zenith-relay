use super::super::{MAX_RATE_LIMIT_RETRY_HINT_MS, upstream_error_text};
use crate::scheduler::ProviderClock;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RateLimitBodyHint {
    pub(crate) retry_clock: Option<ProviderClock>,
    pub(crate) explicit_retry_clock: Option<ProviderClock>,
    pub(crate) headers_normalized: bool,
    pub(crate) chatgpt_subscription: bool,
    /// Exact SIWC quota discriminator, never inferred from diagnostic prose.
    pub(crate) siwc_usage_limit: bool,
    /// A recognized transient discriminator, distinct from an unknown 429.
    pub(crate) explicit_transient_rejection: bool,
    pub(crate) global: bool,
    pub(crate) shared_bucket: Option<[u8; 32]>,
}

impl RateLimitBodyHint {
    pub(crate) fn retry_clock_for_account(self, account_route: bool) -> Option<ProviderClock> {
        if self.chatgpt_subscription && !account_route {
            self.explicit_retry_clock
        } else {
            self.retry_clock
        }
    }

    pub(crate) fn with_retry_headers_at(
        mut self,
        headers: &reqwest::header::HeaderMap,
        now: SystemTime,
    ) -> Self {
        // This hint belongs to one rejection. Re-reading its headers must not
        // renew a relative Retry-After while that rejection waits to settle.
        if !self.headers_normalized {
            let observed_at_ms = crate::unix_time_ms_at(now);
            let header_clock = retry_after_ms(headers, now).map(|delay| {
                ProviderClock::new(observed_at_ms.saturating_add(delay), Some(observed_at_ms))
            });
            self.retry_clock =
                ProviderClock::latest(self.retry_clock.into_iter().chain(header_clock));
            self.explicit_retry_clock =
                ProviderClock::latest(self.explicit_retry_clock.into_iter().chain(header_clock));
            self.headers_normalized = true;
        }
        self
    }

    pub(crate) fn merge_retry_clocks(&mut self, other: Self) {
        self.retry_clock =
            ProviderClock::latest(self.retry_clock.into_iter().chain(other.retry_clock));
        self.explicit_retry_clock = ProviderClock::latest(
            self.explicit_retry_clock
                .into_iter()
                .chain(other.explicit_retry_clock),
        );
        self.headers_normalized |= other.headers_normalized;
    }
}

pub(crate) fn rate_limit_body_hint(response_body: &[u8]) -> RateLimitBodyHint {
    rate_limit_body_hint_at(response_body, SystemTime::now())
}

pub(crate) fn rate_limit_body_hint_at(response_body: &[u8], now: SystemTime) -> RateLimitBodyHint {
    let Some(rate_limit_payload) = std::str::from_utf8(response_body)
        .ok()
        .and_then(|body| crate::strict_json::parse(body).ok())
    else {
        return RateLimitBodyHint::default();
    };
    rate_limit_body_hint_value(&rate_limit_payload, now)
}

pub(crate) fn rate_limit_body_hint_value(
    rate_limit_payload: &Value,
    now: SystemTime,
) -> RateLimitBodyHint {
    let explicit_retry_after_ms = [
        "/retry_after",
        "/error/retry_after",
        "/body/error/retry_after",
        "/response/error/retry_after",
    ]
    .into_iter()
    .find_map(|path| {
        rate_limit_payload
            .pointer(path)
            .and_then(json_seconds_to_ms)
    })
    .or_else(|| retry_delay_from_text(&upstream_error_text(rate_limit_payload)));
    let retry_after_ms = rate_limit_reset_delay_ms(rate_limit_payload, now)
        .or_else(|| {
            [
                "/resets_in_seconds",
                "/error/resets_in_seconds",
                "/body/error/resets_in_seconds",
                "/response/error/resets_in_seconds",
            ]
            .into_iter()
            .find_map(|path| {
                rate_limit_payload
                    .pointer(path)
                    .and_then(json_seconds_to_ms)
            })
        })
        .or(explicit_retry_after_ms);
    let siwc_usage_limit = super::super::classify::siwc_error_code(rate_limit_payload)
        == Some("subscription_sharing_usage_limit_exceeded");
    let chatgpt_subscription = siwc_usage_limit
        || ["", "/error", "/body/error", "/response/error"]
            .into_iter()
            .filter_map(|path| rate_limit_payload.pointer(path))
            .any(|error| {
                ["code", "type"].into_iter().any(|field| {
                    matches!(
                        error.get(field).and_then(Value::as_str),
                        Some("usage_limit_reached")
                    )
                })
            });
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
    .filter_map(|path| rate_limit_payload.pointer(path).and_then(Value::as_str))
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
    let observed_at_ms = crate::unix_time_ms_at(now);
    let error = ["/error", "/response/error", "/body/error"]
        .into_iter()
        .find_map(|path| rate_limit_payload.pointer(path))
        .unwrap_or(rate_limit_payload);
    let explicit_transient_rejection = matches!(
        error
            .get("code")
            .or_else(|| error.get("type"))
            .and_then(Value::as_str),
        Some("rate_limit_exceeded" | "slow_down" | "server_is_overloaded")
    );
    let clock =
        |delay| ProviderClock::new(observed_at_ms.saturating_add(delay), Some(observed_at_ms));
    RateLimitBodyHint {
        retry_clock: retry_after_ms.map(clock),
        explicit_retry_clock: explicit_retry_after_ms.map(clock),
        headers_normalized: false,
        chatgpt_subscription,
        siwc_usage_limit,
        explicit_transient_rejection,
        global,
        shared_bucket: organization_rate_limit_bucket(rate_limit_payload),
    }
}

fn organization_rate_limit_bucket(payload: &Value) -> Option<[u8; 32]> {
    let error = ["/error", "/body/error", "/response/error", ""]
        .into_iter()
        .filter_map(|path| payload.pointer(path))
        .find(|error| error["code"].as_str() == Some("rate_limit_exceeded"))?;
    let dimension = error["type"].as_str()?;
    let unit = match dimension {
        "tokens" => "tokens per min",
        "requests" => "requests per min",
        _ => return None,
    };
    let message = error["message"].as_str()?;
    let (model, remainder) = message
        .strip_prefix("Rate limit reached for ")?
        .split_once(" in organization ")?;
    let (organization, limit) = remainder.split_once(" on ")?;
    if !crate::catalog::is_valid_model_token(model)
        || organization.is_empty()
        || organization.len() > 128
        || !organization
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || !limit.starts_with(unit)
    {
        return None;
    }
    // Keep quota identity opaque; account names and provider messages are not stored here.
    let mut digest = Sha256::new();
    for value in [organization, model, dimension] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    Some(digest.finalize().into())
}

fn rate_limit_reset_delay_ms(rate_limit_payload: &Value, now: SystemTime) -> Option<u64> {
    let reset_at = [
        "/resets_at",
        "/error/resets_at",
        "/body/error/resets_at",
        "/response/error/resets_at",
    ]
    .into_iter()
    .find_map(|path| rate_limit_payload.pointer(path).and_then(json_u64))?;
    let reset_at_ms = if reset_at > 10_000_000_000 {
        reset_at
    } else {
        reset_at.checked_mul(1_000)?
    };
    let now_ms = u64::try_from(now.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()?;
    reset_at_ms
        .checked_sub(now_ms)
        .filter(|duration_ms| *duration_ms > 0)
        .map(|duration_ms| duration_ms.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

fn json_u64(json_value: &Value) -> Option<u64> {
    json_value.as_u64().or_else(|| {
        json_value
            .as_str()
            .and_then(|text| text.trim().parse().ok())
    })
}

fn json_seconds_to_ms(duration_value: &Value) -> Option<u64> {
    let seconds = duration_value.as_f64().or_else(|| {
        duration_value
            .as_str()
            .and_then(|text| text.trim().parse().ok())
    })?;
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

pub(crate) fn retry_after_ms(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<u64> {
    crate::transport::retry_after_ms(headers, now)
        .map(|delay| delay.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

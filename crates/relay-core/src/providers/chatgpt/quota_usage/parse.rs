use crate::error_codes;
use crate::quota::{
    QuotaRefreshData, QuotaRefreshFailure, QuotaRefreshResult, QuotaWindowInput, QuotaWindowKind,
    ResetTime, SubscriptionInput, SupplementalQuotaWindowInput,
};
use crate::DefaultServiceTier;
use serde::Deserialize;

const MAX_ADDITIONAL_LIMITS: usize = 15;
const CREDIT_MICRO_UNITS: f64 = 1_000_000.0;
// Keep the serialized micro-unit value exactly representable by JavaScript.
const MAX_AVAILABLE_CREDITS: f64 = 9_000_000_000.0;

#[derive(Deserialize)]
struct UsagePayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimitStatus>,
    #[serde(default)]
    code_review_rate_limit: Option<SupplementalRateLimitStatus>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<AdditionalRateLimitStatus>>,
    #[serde(default)]
    rate_limit_reset_credits: Option<ResetCreditsSummary>,
    /// Provider ledgers are intentionally parsed from their explicit fields
    /// below. Malformed optional credit data must not fail a quota refresh.
    #[serde(default)]
    credits: serde_json::Value,
    #[serde(default)]
    rate_limit_reached_type: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RateLimitStatus {
    #[serde(default)]
    allowed: Option<bool>,
    #[serde(default)]
    limit_reached: Option<bool>,
    #[serde(default)]
    primary_window: Option<RateLimitWindow>,
    #[serde(default)]
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Clone, Deserialize)]
struct RateLimitWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct SupplementalRateLimitStatus {
    #[serde(default)]
    primary_window: Option<RateLimitWindow>,
    #[serde(default)]
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct AdditionalRateLimitStatus {
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    metered_feature: Option<String>,
    #[serde(default)]
    rate_limit: Option<SupplementalRateLimitStatus>,
}

#[derive(Deserialize)]
struct ResetCreditsSummary {
    #[serde(default, alias = "availableCount")]
    available_count: Option<ResetCreditCount>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResetCreditCount {
    Integer(i64),
    Text(String),
}

impl ResetCreditCount {
    fn into_u32(self) -> Option<u32> {
        match self {
            Self::Integer(value) => u32::try_from(value).ok(),
            Self::Text(value) => value.trim().parse().ok(),
        }
    }
}

pub fn parse_codex_usage(
    body: &[u8],
    observed_at_ms: u64,
) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
    let payload: UsagePayload = serde_json::from_slice(body)
        .map_err(|_| QuotaRefreshFailure::new(error_codes::QUOTA_INVALID_RESPONSE, false))?;
    let supplemental = collect_supplemental_windows(&payload, observed_at_ms);
    let provider_credits = provider_credits(&payload);
    let explicit_limit_reached = payload.rate_limit_reached_type.is_some();
    let (primary, secondary, allowed, limit_reached) = match payload.rate_limit {
        Some(rate_limit) => (
            rate_limit
                .primary_window
                .map(|window| map_window(window, QuotaWindowKind::Primary, observed_at_ms))
                .transpose()?,
            rate_limit
                .secondary_window
                .map(|window| map_window(window, QuotaWindowKind::Secondary, observed_at_ms))
                .transpose()?,
            rate_limit.allowed,
            rate_limit.limit_reached,
        ),
        None => (None, None, None, None),
    };
    let limit_reached = explicit_limit_reached.then_some(true).or(limit_reached);
    let subscription = payload
        .plan_type
        .as_deref()
        .and_then(safe_label)
        .map(|plan_type| SubscriptionInput {
            plan_type: Some(plan_type),
            active_until_ms: None,
            forbidden: false,
            observed_at_ms,
        });
    Ok(QuotaRefreshResult {
        quota: QuotaRefreshData {
            primary,
            secondary,
            supplemental,
            limit_reached: limit_reached == Some(true),
            subscription,
            reset_credits_available: payload
                .rate_limit_reset_credits
                .and_then(|credits| credits.available_count)
                .and_then(ResetCreditCount::into_u32),
            available_credits_micro_units: provider_credits.micro_units,
            provider_credits_available: provider_credits.available,
            provider_credits_unlimited: provider_credits.unlimited,
            direct_balance_micro_usd: None,
            observed_at_ms,
        },
        allowed,
        reported_limit_reached: limit_reached,
    })
}

#[derive(Clone, Copy, Default)]
struct ProviderCredits {
    micro_units: Option<u64>,
    available: bool,
    unlimited: bool,
}

/// Extracts only documented provider credit ledgers. A positive or unlimited
/// ledger is fresh evidence that the account can run despite exhausted rate
/// windows; an absent or malformed ledger makes no routing claim.
fn provider_credits(payload: &UsagePayload) -> ProviderCredits {
    let mut result = ProviderCredits::default();
    // `spend_control.individual_limit` is a separate spending-control
    // configuration. It is not a credit ledger and may remain at a static
    // ceiling while `credits.remaining` decreases. Do not display, aggregate,
    // or use it as credit availability.
    match &payload.credits {
        serde_json::Value::Object(credits) => {
            result.record_unlimited(json_bool(credits.get("unlimited")));
            result.record_amount(
                json_number(credits.get("remaining"))
                    .or_else(|| json_number(credits.get("balance"))),
            );
        }
        serde_json::Value::Array(credits) => {
            let (total, found) = credits.iter().fold((0.0, false), |(total, found), credit| {
                let amount = credit
                    .as_object()
                    // This legacy array shape is numeric in the provider
                    // contract. Do not coerce arbitrary strings here: an
                    // invalid legacy entry must not make an account eligible.
                    .and_then(|credit| credit.get("credit_amount"))
                    .and_then(serde_json::Value::as_f64);
                match amount {
                    Some(amount) if valid_credit_amount(amount) => (total + amount, true),
                    _ => (total, found),
                }
            });
            if found && valid_credit_amount(total) {
                result.record_amount(Some(total));
            }
        }
        _ => {}
    }
    result
}

impl ProviderCredits {
    fn record_unlimited(&mut self, unlimited: Option<bool>) {
        if unlimited == Some(true) {
            self.available = true;
            self.unlimited = true;
        }
    }

    fn record_amount(&mut self, amount: Option<f64>) {
        let Some(amount) = amount.filter(|amount| valid_credit_amount(*amount)) else {
            return;
        };
        let micro_units = (amount * CREDIT_MICRO_UNITS).round() as u64;
        self.micro_units = Some(micro_units);
        self.available |= amount > 0.0;
    }
}

fn valid_credit_amount(amount: f64) -> bool {
    amount.is_finite() && (0.0..=MAX_AVAILABLE_CREDITS).contains(&amount)
}

fn json_number(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(value) => value.as_f64(),
        serde_json::Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

fn json_bool(value: Option<&serde_json::Value>) -> Option<bool> {
    match value? {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn collect_supplemental_windows(
    payload: &UsagePayload,
    observed_at_ms: u64,
) -> Vec<SupplementalQuotaWindowInput> {
    let mut windows = Vec::new();
    if let Some(rate_limit) = payload.code_review_rate_limit.as_ref() {
        append_supplemental_windows(
            &mut windows,
            "code_review",
            "Code Review",
            rate_limit,
            observed_at_ms,
        );
    }
    for (index, entry) in payload
        .additional_rate_limits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .take(MAX_ADDITIONAL_LIMITS)
        .enumerate()
    {
        if is_spark_limit(entry) {
            continue;
        }
        let Some(rate_limit) = entry.rate_limit.as_ref() else {
            continue;
        };
        let label = entry
            .limit_name
            .as_deref()
            .and_then(safe_display_label)
            .or_else(|| {
                entry
                    .metered_feature
                    .as_deref()
                    .and_then(safe_display_label)
            })
            .unwrap_or_else(|| "Additional quota".to_string());
        append_supplemental_windows(
            &mut windows,
            &format!("additional:{index}"),
            &label,
            rate_limit,
            observed_at_ms,
        );
    }
    windows
}

fn is_spark_limit(entry: &AdditionalRateLimitStatus) -> bool {
    entry
        .limit_name
        .as_deref()
        .into_iter()
        .chain(entry.metered_feature.as_deref())
        .any(|value| value.to_ascii_lowercase().contains("spark"))
}

fn append_supplemental_windows(
    output: &mut Vec<SupplementalQuotaWindowInput>,
    id_prefix: &str,
    label: &str,
    rate_limit: &SupplementalRateLimitStatus,
    observed_at_ms: u64,
) {
    let service_tier = supplemental_service_tier(label);
    for (kind, window) in [
        (QuotaWindowKind::Primary, rate_limit.primary_window.as_ref()),
        (
            QuotaWindowKind::Secondary,
            rate_limit.secondary_window.as_ref(),
        ),
    ] {
        let Some(window) = window else { continue };
        let kind_label = match kind {
            QuotaWindowKind::Primary => "primary",
            QuotaWindowKind::Secondary => "secondary",
        };
        let Ok(window) = map_window(window.clone(), kind, observed_at_ms) else {
            continue;
        };
        output.push(SupplementalQuotaWindowInput {
            id: format!("{id_prefix}:{kind_label}"),
            label: label.to_string(),
            service_tier,
            window,
        });
    }
}

fn supplemental_service_tier(label: &str) -> Option<DefaultServiceTier> {
    label
        .split(|character: char| !character.is_ascii_alphanumeric())
        .find_map(|word| {
            if word.eq_ignore_ascii_case("ultrafast") {
                Some(DefaultServiceTier::Ultrafast)
            } else if word.eq_ignore_ascii_case("priority") || word.eq_ignore_ascii_case("fast") {
                Some(DefaultServiceTier::Fast)
            } else {
                None
            }
        })
}

fn map_window(
    window: RateLimitWindow,
    kind: QuotaWindowKind,
    observed_at_ms: u64,
) -> Result<QuotaWindowInput, QuotaRefreshFailure> {
    let used_percent = window
        .used_percent
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        .ok_or_else(|| QuotaRefreshFailure::new(error_codes::QUOTA_INVALID_PERCENTAGE, false))?;
    let reset = window
        .reset_at
        .filter(|value| *value > 0)
        .and_then(|value| u64::try_from(value).ok())
        .map(ResetTime::AbsoluteUnixSeconds)
        .or_else(|| {
            window
                .reset_after_seconds
                .filter(|value| *value >= 0)
                .and_then(|value| u64::try_from(value).ok())
                .map(ResetTime::RelativeSeconds)
        });
    Ok(QuotaWindowInput {
        kind,
        available_percent: Some(100.0 - used_percent),
        explicitly_full: None,
        reset,
        window_minutes: window
            .limit_window_seconds
            .filter(|seconds| *seconds > 0)
            .and_then(|seconds| u32::try_from((seconds.saturating_add(59)) / 60).ok()),
        provider_cycle_id: None,
        observed_at_ms,
    })
}

fn safe_label(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_lowercase())
}

fn safe_display_label(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control))
        .then(|| value.split_whitespace().collect::<Vec<_>>().join(" "))
}

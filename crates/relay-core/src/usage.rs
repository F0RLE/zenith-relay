mod api_equivalent;
mod event;
mod reasoning;
mod tools;
mod upstream_error;

pub use api_equivalent::{
    estimate_api_equivalent_with_catalog, estimate_api_equivalent_with_token_price,
    estimate_candidate_api_equivalent_with_catalog, normalize_model_price_overrides,
    resolve_candidate_price, ApiEquivalentUsage, ApiModelPriceOverride, ApiModelPriceSources,
    CandidatePriceQuery, CatalogPriceResolver, ObservedUsageSums, SourceModelPriceOverrides,
    UsageContextBand, UsagePriceClass, API_EQUIVALENT_AGGREGATE_SQL,
    CACHE_WRITE_TTL_BUCKET_SUMS_SQL, CANDIDATE_ROLLUP_TOKEN_OFFSET,
    PRICED_AGGREGATE_CACHED_INPUT_TOKENS, PRICED_AGGREGATE_CACHED_SAMPLES,
    PRICED_AGGREGATE_CACHE_WRITE_1H_TOKENS, PRICED_AGGREGATE_CACHE_WRITE_5M_TOKENS,
    PRICED_AGGREGATE_CACHE_WRITE_SAMPLES, PRICED_AGGREGATE_INPUT_SAMPLES,
    PRICED_AGGREGATE_INPUT_TOKENS, PRICED_AGGREGATE_OUTPUT_SAMPLES, PRICED_AGGREGATE_OUTPUT_TOKENS,
    PRICED_AGGREGATE_TOTAL_SAMPLES, PRICED_AGGREGATE_TOTAL_TOKENS,
    PRICED_AGGREGATE_UNKNOWN_CACHE_WRITE_TOKENS, USAGE_CONTEXT_BAND_SQL, USAGE_PRICE_CLASS_SQL,
};
pub use event::{ErrorOrigin, UsageEvent, UsageTransport};
pub use reasoning::normalize_reasoning_effort;
pub(crate) use reasoning::ReasoningEffortDiagnostics;
pub use tools::{TerminalOutputKind, ToolChoiceMode, ToolUseDiagnostics};
pub use upstream_error::UpstreamErrorDetails;

/// Escapes a user value for a `LIKE ? ESCAPE '\\'` contains query.
pub fn sql_like_contains_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

/// Stores a counter in SQLite. A value that does not fit in `i64` stays at the top.
pub fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Reads a stored counter. A negative or overflowing value becomes zero.
pub fn sql_count_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

/// `NULL` and a negative stored integer are both absent. Zero stays zero.
pub fn sql_optional_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

pub const DELETE_ACCOUNT_CANDIDATE_ROLLUPS_SQL: &str =
    "DELETE FROM usage_candidate_rollups WHERE candidate_kind = 'account' AND candidate_id = ?1";

#[cfg(test)]
mod sql_int_tests {
    use super::sql_optional_u64;

    #[test]
    fn null_and_negative_measurements_stay_absent() {
        assert_eq!(sql_optional_u64(None), None);
        assert_eq!(sql_optional_u64(Some(-1)), None);
        assert_eq!(sql_optional_u64(Some(0)), Some(0));
        assert_eq!(sql_optional_u64(Some(12)), Some(12));
    }
}

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type UsageCallback = Arc<dyn Fn(UsageEvent) + Send + Sync>;

/// A provider-neutral monetary value attached to measured token usage.
///
/// The OpenAI catalog is one way to produce this value; the estimate does not
/// imply that the provider charged or debited the same amount.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageValue {
    pub micro_usd: u64,
    pub priced_tokens: u64,
    pub unpriced_tokens: u64,
}

impl UsageValue {
    pub fn merge(&mut self, other: Self) {
        self.micro_usd = self.micro_usd.saturating_add(other.micro_usd);
        self.priced_tokens = self.priced_tokens.saturating_add(other.priced_tokens);
        self.unpriced_tokens = self.unpriced_tokens.saturating_add(other.unpriced_tokens);
    }
}

/// Compatibility name used by the management and desktop DTOs. New provider
/// code should use `UsageValue` so it does not imply an OpenAI-only source.
pub type ApiEquivalentSummary = UsageValue;

/// A safe, provider-reported service-tier diagnostic.
///
/// It intentionally remains separate from [`DefaultServiceTier`], which is
/// Relay's Normal/Fast pool policy. Upstreams can add tier names, so Relay
/// stores safe normalized text instead of silently discarding a new value.
pub type ObservedServiceTier = String;

pub fn normalize_observed_service_tier(value: &str) -> Option<ObservedServiceTier> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 48
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

/// Validates and normalizes provider-reported cache-window durations.
/// Multiple values are kept in ascending duration order, for example
/// `"5m, 1h"`. Unknown or malformed values stay unreported in the UI.
pub fn normalize_reported_cache_ttls(value: &str) -> Option<String> {
    let mut windows = Vec::new();
    for raw in value.split([',', '+']) {
        let raw = raw.trim().to_ascii_lowercase();
        let (amount, unit) = ["ms", "s", "m", "h", "d"]
            .into_iter()
            .find_map(|unit| raw.strip_suffix(unit).map(|amount| (amount, unit)))?;
        let amount = amount.parse::<u32>().ok()?;
        if amount == 0 {
            return None;
        }
        let multiplier = match unit {
            "ms" => 1_u64,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            _ => return None,
        };
        let duration_ms = u64::from(amount).checked_mul(multiplier)?;
        if !windows.iter().any(|(duration, _)| *duration == duration_ms) {
            windows.push((duration_ms, format!("{amount}{unit}")));
        }
        if windows.len() > 8 {
            return None;
        }
    }
    if windows.is_empty() {
        return None;
    }
    windows.sort_by_key(|(duration, _)| *duration);
    Some(
        windows
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Normalized provider clock evidence, independent of local recovery policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBackoff {
    pub scope: String,
    pub not_before_ms: u64,
    /// Local clock used to normalize the provider hint, not a storage timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ProviderClock {
    pub(super) not_before_ms: u64,
    pub(super) observed_at_ms: Option<u64>,
}

impl ProviderClock {
    pub(super) fn new(not_before_ms: u64, observed_at_ms: Option<u64>) -> Self {
        Self {
            not_before_ms,
            observed_at_ms: observed_at_ms.filter(|observed| *observed < not_before_ms),
        }
    }

    pub(super) fn latest(clocks: impl Iterator<Item = Self>) -> Option<Self> {
        // Keep the original known observation when the selected deadline is unchanged.
        clocks.max_by_key(|clock| {
            (
                clock.not_before_ms,
                std::cmp::Reverse(clock.observed_at_ms.unwrap_or(u64::MAX)),
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CooldownMetadata {
    pub(super) reason: Option<CooldownReason>,
    pub(super) provider_clock: Option<ProviderClock>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CooldownReason {
    Transient,
    RateLimit,
    QuotaExhausted,
    Mandatory,
}

impl CooldownReason {
    pub(crate) fn is_rate_limit(self) -> bool {
        matches!(self, Self::RateLimit | Self::QuotaExhausted)
    }
}

pub(super) fn active_retry_at(
    cooldowns: &BTreeMap<String, u64>,
    model: &str,
    now_ms: u64,
) -> Option<u64> {
    cooldowns
        .iter()
        .filter(|(candidate_model, retry_at)| {
            (**retry_at > now_ms)
                && (candidate_model.as_str() == "*" || candidate_model.eq_ignore_ascii_case(model))
        })
        .map(|(_, retry_at)| *retry_at)
        .max()
}

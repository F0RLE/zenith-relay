mod refresh;
mod windows;

pub(crate) fn observation_is_fresh(
    observed_at_ms: Option<u64>,
    now_ms: u64,
    stale_after_ms: u64,
) -> bool {
    observed_at_ms
        .and_then(|observed_at_ms| now_ms.checked_sub(observed_at_ms))
        .is_some_and(|age_ms| age_ms <= stale_after_ms)
}

pub use refresh::{
    classify_quota_http_failure, subscription_plan_changed, QuotaAdapter, QuotaAdapterCapabilities,
    QuotaAdapterContext, QuotaRefreshData, QuotaRefreshFailure, QuotaRefreshResult,
    SupplementalQuotaWindowInput,
};
pub(crate) use windows::normalize_subscription_plan;
pub use windows::{
    QuotaErrorState, QuotaNormalizationError, QuotaSnapshot, QuotaTransition, QuotaWindow,
    QuotaWindowInput, QuotaWindowKind, ResetTime, Subscription, SubscriptionInput,
    SubscriptionStatus, SupplementalQuotaWindow,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodexSubscriptionMetadata {
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub active_until_ms: Option<u64>,
}

pub const SUBSCRIPTION_REFRESH_INTERVAL_MS: u64 = 30 * 60 * 1_000;

pub fn subscription_refresh_due(
    active_until_ms: Option<u64>,
    updated_at_ms: Option<u64>,
    now_ms: u64,
) -> bool {
    if active_until_ms.is_none() {
        return true;
    }
    updated_at_ms
        .map(|updated_at_ms| {
            now_ms.saturating_sub(updated_at_ms) >= SUBSCRIPTION_REFRESH_INTERVAL_MS
        })
        .unwrap_or(true)
}

pub fn merge_subscription_metadata(
    plan_type: &mut Option<String>,
    active_until_ms: &mut Option<u64>,
    metadata: CodexSubscriptionMetadata,
) {
    merge_subscription_metadata_at(plan_type, active_until_ms, metadata, None);
}

pub fn merge_subscription_metadata_at(
    plan_type: &mut Option<String>,
    active_until_ms: &mut Option<u64>,
    metadata: CodexSubscriptionMetadata,
    observed_at_ms: Option<u64>,
) {
    let plan_changed = crate::quota::subscription_plan_changed(
        plan_type.as_deref(),
        metadata.plan_type.as_deref(),
    );
    if plan_changed && metadata.active_until_ms.is_none() {
        *active_until_ms = None;
    }
    if metadata.plan_type.is_some() {
        *plan_type = metadata.plan_type.clone();
    }
    if metadata.active_until_ms.is_some() {
        *active_until_ms = metadata.active_until_ms;
    } else if metadata.plan_type.is_some()
        && !plan_changed
        && observed_at_ms.is_some_and(|now_ms| {
            active_until_ms.is_some_and(|active_until_ms| active_until_ms <= now_ms)
        })
    {
        // A successful probe that confirms the same plan but no longer
        // returns the old expiry must not leave a stale Expired state.
        *active_until_ms = None;
    }
}

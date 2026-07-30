//! Valuation seam between the API product layer and account quota learning.
//!
//! Account quota is learned in a *reference* unit of account: the published
//! catalog price of the tokens that consumed the quota. That unit must stay
//! comparable across accounts, plans and operators, so this layer deliberately
//! cannot see operator pricing.
//!
//! Operator model price overrides describe what the API product costs or sells
//! for. They belong to billing, telemetry and the ledger, and are applied by
//! `estimate_api_equivalent_with_price_override` on that side. Feeding them in
//! here would make two accounts with identical real quota appear to hold
//! different capacity, and would silently change the meaning of already stored
//! calibration samples, because [`quota_valuation_revision`] describes the
//! catalog and not per-operator tuning.
use crate::{api_pricing_revision, estimate_api_equivalent, UsageEvent, UsageValue};
use std::sync::OnceLock;

/// Values a measured usage event for quota calibration.
///
/// Takes no price override by design: see the module comment.
pub fn quota_reference_value(event: &UsageEvent) -> UsageValue {
    estimate_api_equivalent(
        event
            .resolved_model
            .as_deref()
            .or(event.requested_model.as_deref()),
        event.input_tokens,
        event.cached_input_tokens,
        event.cache_write_input_tokens,
        event.output_tokens,
        event.total_tokens,
    )
}

/// Identity of the valuation formula above. A change invalidates stored
/// calibration samples, because they are denominated in this unit.
pub fn quota_valuation_revision() -> &'static str {
    static REVISION: OnceLock<String> = OnceLock::new();
    REVISION
        .get_or_init(|| format!("api:{}", api_pricing_revision()))
        .as_str()
}

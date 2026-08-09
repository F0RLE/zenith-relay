use super::*;

impl QuotaEconomicsState {
    pub fn purchase_cost_micro_usd(&self) -> Option<u64> {
        self.purchase_cost_micro_usd
    }

    pub fn set_purchase_cost_micro_usd(&mut self, value: Option<u64>) {
        self.purchase_cost_micro_usd = value;
    }

    pub fn observe_event(&mut self, event: &crate::UsageEvent, usage_value: UsageValue) {
        let observed_at_ms = event
            .quota_snapshot
            .as_ref()
            .and_then(|quota| quota.updated_at_ms)
            .unwrap_or_else(|| self.latest_observed_at_ms());
        self.observe_event_at(event, usage_value, observed_at_ms);
    }

    pub fn observe_event_at(
        &mut self,
        event: &crate::UsageEvent,
        usage_value: UsageValue,
        observed_at_ms: u64,
    ) {
        if let Some(usage) = QuotaEconomicsUsage::from_event(event, usage_value) {
            self.observe_usage_with_source(
                usage,
                event.quota_snapshot.as_ref(),
                observed_at_ms,
                QuotaObservationSource::Passive,
            );
        } else if let Some(quota) = event.quota_snapshot.as_ref() {
            self.observe_quota_with_source(quota, QuotaObservationSource::Passive);
        }
    }

    pub fn observe_usage(&mut self, usage: QuotaEconomicsUsage, quota: Option<&QuotaSnapshot>) {
        let observed_at_ms = quota
            .and_then(|quota| quota.updated_at_ms)
            .unwrap_or_else(|| self.latest_observed_at_ms());
        self.observe_usage_with_source(
            usage,
            quota,
            observed_at_ms,
            QuotaObservationSource::Active,
        );
    }

    pub(super) fn observe_usage_with_source(
        &mut self,
        usage: QuotaEconomicsUsage,
        quota: Option<&QuotaSnapshot>,
        observed_at_ms: u64,
        source: QuotaObservationSource,
    ) {
        self.primary.record_usage(usage, observed_at_ms);
        self.secondary.record_usage(usage, observed_at_ms);
        if let Some(quota) = quota {
            self.observe_quota_with_source(quota, source);
        }
    }

    pub fn observe_quota(&mut self, quota: &QuotaSnapshot) {
        self.observe_quota_with_source(quota, QuotaObservationSource::Active);
    }

    pub(super) fn observe_quota_with_source(
        &mut self,
        quota: &QuotaSnapshot,
        source: QuotaObservationSource,
    ) {
        let context = EconomicsContext {
            provider: self.provider.clone(),
            plan: self.plan.clone(),
            pricing_revision: self.pricing_revision.clone(),
        };
        let limiting_available = quota
            .primary
            .iter()
            .chain(quota.secondary.iter())
            .filter_map(|window| window.available_basis_points)
            .min();
        let all_windows_are_known = quota
            .primary
            .iter()
            .chain(quota.secondary.iter())
            .all(|window| window.available_basis_points.is_some());
        if let Some(window) = quota.primary.as_ref() {
            self.primary.observe_quota(
                window,
                &context,
                source,
                quota.limit_reached
                    && all_windows_are_known
                    && window.available_basis_points == limiting_available,
            );
        }
        if let Some(window) = quota.secondary.as_ref() {
            self.secondary.observe_quota(
                window,
                &context,
                source,
                quota.limit_reached
                    && all_windows_are_known
                    && window.available_basis_points == limiting_available,
            );
        }
    }

    fn latest_observed_at_ms(&self) -> u64 {
        self.primary
            .observations
            .last()
            .into_iter()
            .chain(self.secondary.observations.last())
            .map(|observation| observation.observed_at_ms)
            .max()
            .unwrap_or_default()
    }

    pub fn set_account_context(&mut self, provider: &str, plan: Option<&str>) {
        let provider = provider.trim().to_ascii_lowercase();
        let plan = plan
            .map(crate::quota::windows::normalize_subscription_plan)
            .filter(|value| !value.is_empty());
        if self.provider == provider && self.plan == plan {
            return;
        }
        self.provider = provider;
        self.plan = plan;
        self.primary.begin_new_epoch();
        self.secondary.begin_new_epoch();
    }

    pub fn reset_learning(&mut self) {
        self.primary = WindowEconomicsHistory::default();
        self.secondary = WindowEconomicsHistory::default();
        self.pricing_revision = None;
    }

    pub fn reset_learning_for_revision(&mut self, revision: &str) {
        self.primary.begin_new_epoch();
        self.secondary.begin_new_epoch();
        let revision = revision.trim();
        if !revision.is_empty() {
            self.pricing_revision = Some(revision.to_string());
        }
    }

    /// Selects the valuation formula used by the provider that feeds this
    /// state. A changed revision invalidates old calibration samples.
    pub fn set_value_revision(&mut self, revision: &str) {
        let revision = revision.trim();
        if revision.is_empty() {
            return;
        }
        if self.pricing_revision.as_deref() != Some(revision) {
            self.reset_learning_for_revision(revision);
        }
    }
}

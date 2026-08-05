use super::{QuotaSnapshot, QuotaWindow, QuotaWindowKind};
use crate::{DefaultServiceTier, UsageValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_INTERVAL_SAMPLES: usize = 12;
const MAX_DRIFT_SAMPLES: usize = 6;
const MAX_CYCLE_RECORDS: usize = 12;
const MAX_QUOTA_OBSERVATIONS: usize = 48;
const MIN_ESTIMATE_BASIS_POINTS: u64 = 100;
const DRIFT_BASELINE_SAMPLES: usize = 3;
const DRIFT_CONFIRMATION_BASIS_POINTS: u64 = 500;
const DRIFT_THRESHOLD_BASIS_POINTS: u64 = 3_500;
const MEDIUM_CONFIDENCE_OBSERVED_BASIS_POINTS: u64 = 1_000;
const MEDIUM_CONFIDENCE_IQR_BPS: u64 = 5_000;
const ACTIVE_ATTRIBUTION_MAX_GAP_MS: u64 = 60_000;
const RESET_SKEW_MS: u64 = 60_000;
const FULL_CYCLE_THRESHOLD_BASIS_POINTS: u16 = 9_950;
const EXHAUSTED_CYCLE_THRESHOLD_BASIS_POINTS: u16 = 50;
pub const MAX_PURCHASE_COST_MICRO_USD: u64 = 1_000_000_000_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsUsage {
    pub api_equivalent_micro_usd: Option<u64>,
    pub requests: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub service_tier: DefaultServiceTier,
}

impl QuotaEconomicsUsage {
    pub fn from_event(event: &crate::UsageEvent, usage_value: UsageValue) -> Option<Self> {
        let input_tokens = event.input_tokens.unwrap_or_default();
        let output_tokens = event.output_tokens.unwrap_or_default();
        let total_tokens = event
            .total_tokens
            .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
        // The requested ChatGPT mode determines quota consumption. The response
        // service_tier is retained separately as delivery telemetry.
        let service_tier = event.service_tier;
        (event.success || total_tokens > 0 || usage_value.priced_tokens > 0).then_some(Self {
            api_equivalent_micro_usd: (usage_value.unpriced_tokens == 0)
                .then_some(usage_value.micro_usd),
            requests: u64::from(event.success),
            input_tokens,
            cached_input_tokens: event.cached_input_tokens.unwrap_or_default(),
            reasoning_tokens: event.reasoning_tokens.unwrap_or_default(),
            output_tokens,
            total_tokens,
            service_tier,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaObservationSource {
    #[default]
    Active,
    Passive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaObservationRecord {
    pub window_kind: QuotaWindowKind,
    pub used_basis_points: Option<u16>,
    pub available_basis_points: Option<u16>,
    pub delta_basis_points: i32,
    pub resolution_basis_points: u16,
    pub reset_at_ms: Option<u64>,
    pub window_minutes: Option<u32>,
    pub observed_at_ms: u64,
    pub source: QuotaObservationSource,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsState {
    pub purchase_cost_micro_usd: Option<u64>,
    #[serde(default)]
    pricing_revision: Option<String>,
    #[serde(default)]
    primary: WindowEconomicsHistory,
    #[serde(default, alias = "weekly")]
    secondary: WindowEconomicsHistory,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    plan: Option<String>,
}

impl QuotaEconomicsState {
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

    fn observe_usage_with_source(
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

    fn observe_quota_with_source(&mut self, quota: &QuotaSnapshot, source: QuotaObservationSource) {
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
        if let Some(window) = quota.primary.as_ref() {
            self.primary.observe_quota(
                window,
                &context,
                source,
                quota.limit_reached && window.available_basis_points == limiting_available,
            );
        }
        if let Some(window) = quota.secondary.as_ref() {
            self.secondary.observe_quota(
                window,
                &context,
                source,
                quota.limit_reached && window.available_basis_points == limiting_available,
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
            .map(super::windows::normalize_subscription_plan)
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

#[derive(Clone, Debug, Default)]
struct EconomicsContext {
    provider: String,
    plan: Option<String>,
    pricing_revision: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowEconomicsHistory {
    last_available_basis_points: Option<u16>,
    last_reset_at_ms: Option<u64>,
    last_window_minutes: Option<u32>,
    #[serde(default)]
    pending: PendingUsage,
    #[serde(default)]
    calibration_pending: PendingCalibration,
    #[serde(default)]
    samples: Vec<IntervalSample>,
    #[serde(default)]
    drift_samples: Vec<IntervalSample>,
    #[serde(default)]
    epoch: u32,
    #[serde(default = "default_observed_resolution_basis_points")]
    observed_resolution_basis_points: u16,
    #[serde(default)]
    active_cycle: Option<ActiveQuotaCycle>,
    #[serde(default)]
    cycles: Vec<QuotaCycleRecord>,
    #[serde(default)]
    observations: Vec<QuotaObservationRecord>,
}

impl Default for WindowEconomicsHistory {
    fn default() -> Self {
        Self {
            last_available_basis_points: None,
            last_reset_at_ms: None,
            last_window_minutes: None,
            pending: PendingUsage::default(),
            calibration_pending: PendingCalibration::default(),
            samples: Vec::new(),
            drift_samples: Vec::new(),
            epoch: 0,
            observed_resolution_basis_points: default_observed_resolution_basis_points(),
            active_cycle: None,
            cycles: Vec::new(),
            observations: Vec::new(),
        }
    }
}

fn default_observed_resolution_basis_points() -> u16 {
    100
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaCycleStatus {
    Complete,
    Censored,
    Contaminated,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaCycleRecord {
    pub status: QuotaCycleStatus,
    pub provider: String,
    pub plan: Option<String>,
    pub window_kind: QuotaWindowKind,
    pub window_minutes: Option<u32>,
    pub fingerprint: String,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub reset_at_ms: Option<u64>,
    pub pricing_revision: Option<String>,
    pub epoch: u32,
    pub service_tier: Option<DefaultServiceTier>,
    #[serde(default)]
    pub standard_observations: u64,
    #[serde(default)]
    pub fast_observations: u64,
    #[serde(default)]
    pub active_observations: u64,
    #[serde(default)]
    pub passive_observations: u64,
    pub consumed_basis_points: u16,
    pub unattributed_basis_points: u16,
    pub api_equivalent_micro_usd: Option<u64>,
    pub requests: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActiveQuotaCycle {
    provider: String,
    plan: Option<String>,
    window_kind: Option<QuotaWindowKind>,
    window_minutes: Option<u32>,
    fingerprint: String,
    started_at_ms: u64,
    last_observed_at_ms: u64,
    reset_at_ms: Option<u64>,
    pricing_revision: Option<String>,
    epoch: u32,
    start_available_basis_points: u16,
    last_available_basis_points: u16,
    unattributed_basis_points: u16,
    active_observations: u64,
    passive_observations: u64,
    usage: PendingUsage,
}

impl WindowEconomicsHistory {
    fn record_usage(&mut self, usage: QuotaEconomicsUsage, observed_at_ms: u64) {
        if self.last_available_basis_points.is_some() {
            self.pending.add(usage, observed_at_ms);
        }
        if let Some(cycle) = &mut self.active_cycle {
            cycle.usage.add(usage, observed_at_ms);
        }
    }

    fn observe_quota(
        &mut self,
        window: &QuotaWindow,
        context: &EconomicsContext,
        source: QuotaObservationSource,
        confirmed_exhausted: bool,
    ) {
        let Some(mut available) = window.available_basis_points else {
            self.record_observation(window, source);
            return;
        };
        if confirmed_exhausted {
            available = 0;
        }
        self.observe_resolution(available);
        self.record_observation(window, source);
        let Some(previous) = self.last_available_basis_points else {
            self.reset_baseline(window, available, context, source);
            return;
        };
        self.observe_resolution(previous.abs_diff(available));
        if self.last_window_minutes.is_some()
            && window.window_minutes.is_some()
            && self.last_window_minutes != window.window_minutes
        {
            self.begin_new_epoch();
            self.reset_baseline(window, available, context, source);
            return;
        }
        if self.is_new_cycle(window, available) {
            self.finish_cycle(QuotaCycleStatus::Censored, window.observed_at_ms);
            self.reset_baseline(window, available, context, source);
            return;
        }
        if available > previous {
            if window.is_fully_available() {
                self.finish_cycle(QuotaCycleStatus::Censored, window.observed_at_ms);
                self.reset_baseline(window, available, context, source);
            }
            return;
        }
        if available < previous {
            let consumed = previous - available;
            if self.pending.is_empty() || !self.attribution_is_clean(window, source) {
                self.calibration_pending = PendingCalibration::default();
                if let Some(cycle) = &mut self.active_cycle {
                    cycle.unattributed_basis_points =
                        cycle.unattributed_basis_points.saturating_add(consumed);
                }
            } else if self.pending.totals.api_equivalent_micro_usd.is_some() {
                self.calibration_pending
                    .add(self.pending, consumed, source, window.observed_at_ms);
                if self.calibration_pending.consumed_basis_points
                    >= self.minimum_sample_basis_points()
                {
                    let sample = self.calibration_pending.take_sample();
                    self.record_sample(sample);
                }
            } else {
                self.calibration_pending = PendingCalibration::default();
            }
            self.pending = PendingUsage::default();
        }
        if let Some(cycle) = &mut self.active_cycle {
            cycle.record_observation(source);
            cycle.last_available_basis_points = available;
            cycle.last_observed_at_ms = window.observed_at_ms;
            cycle.reset_at_ms = window.reset_at_ms.or(cycle.reset_at_ms);
        }
        self.last_available_basis_points = Some(available);
        self.last_reset_at_ms = window.reset_at_ms.or(self.last_reset_at_ms);
        self.last_window_minutes = window.window_minutes.or(self.last_window_minutes);
        if available <= EXHAUSTED_CYCLE_THRESHOLD_BASIS_POINTS {
            let status = self
                .active_cycle
                .as_ref()
                .map_or(QuotaCycleStatus::Censored, |cycle| {
                    if cycle.unattributed_basis_points == 0 {
                        QuotaCycleStatus::Complete
                    } else {
                        QuotaCycleStatus::Contaminated
                    }
                });
            self.finish_cycle(status, window.observed_at_ms);
        }
    }

    fn begin_new_epoch(&mut self) {
        let completed_at_ms = self
            .active_cycle
            .as_ref()
            .map_or(0, |cycle| cycle.last_observed_at_ms);
        self.finish_cycle(QuotaCycleStatus::Censored, completed_at_ms);
        self.last_available_basis_points = None;
        self.last_reset_at_ms = None;
        self.last_window_minutes = None;
        self.pending = PendingUsage::default();
        self.calibration_pending = PendingCalibration::default();
        self.samples.clear();
        self.drift_samples.clear();
        self.epoch = self.epoch.saturating_add(1);
    }

    fn is_new_cycle(&self, window: &QuotaWindow, available: u16) -> bool {
        self.last_reset_at_ms.is_some_and(|previous_reset| {
            previous_reset <= window.observed_at_ms
                || window
                    .reset_at_ms
                    .is_some_and(|reset| reset > previous_reset.saturating_add(RESET_SKEW_MS))
        }) || self
            .last_available_basis_points
            .is_some_and(|previous| available > previous.saturating_add(100))
    }

    fn reset_baseline(
        &mut self,
        window: &QuotaWindow,
        available: u16,
        context: &EconomicsContext,
        source: QuotaObservationSource,
    ) {
        self.last_available_basis_points = Some(available);
        self.last_reset_at_ms = window.reset_at_ms;
        self.last_window_minutes = window.window_minutes;
        self.pending = PendingUsage::default();
        self.calibration_pending = PendingCalibration::default();
        self.drift_samples.clear();
        if available >= FULL_CYCLE_THRESHOLD_BASIS_POINTS || window.explicitly_full == Some(true) {
            self.active_cycle = Some(ActiveQuotaCycle {
                provider: context.provider.clone(),
                plan: context.plan.clone(),
                window_kind: Some(window.kind),
                window_minutes: window.window_minutes,
                fingerprint: window
                    .full_transition_fingerprint
                    .clone()
                    .unwrap_or_else(|| {
                        format!(
                            "{:?}:{}:{}:{}",
                            window.kind,
                            window.reset_at_ms.unwrap_or_default(),
                            window.window_minutes.unwrap_or_default(),
                            window.observed_at_ms
                        )
                    }),
                started_at_ms: window.observed_at_ms,
                last_observed_at_ms: window.observed_at_ms,
                reset_at_ms: window.reset_at_ms,
                pricing_revision: context.pricing_revision.clone(),
                epoch: self.epoch,
                start_available_basis_points: available,
                last_available_basis_points: available,
                active_observations: u64::from(source == QuotaObservationSource::Active),
                passive_observations: u64::from(source == QuotaObservationSource::Passive),
                ..Default::default()
            });
        }
    }

    fn attribution_is_clean(&self, window: &QuotaWindow, source: QuotaObservationSource) -> bool {
        source == QuotaObservationSource::Passive
            || (self.pending.last_observed_at_ms > 0
                && window
                    .observed_at_ms
                    .saturating_sub(self.pending.last_observed_at_ms)
                    <= ACTIVE_ATTRIBUTION_MAX_GAP_MS)
    }

    fn record_observation(&mut self, window: &QuotaWindow, source: QuotaObservationSource) {
        let delta_basis_points = self
            .last_available_basis_points
            .zip(window.available_basis_points)
            .map_or(0, |(previous, available)| {
                i32::from(available) - i32::from(previous)
            });
        let observation = QuotaObservationRecord {
            window_kind: window.kind,
            used_basis_points: window
                .available_basis_points
                .map(|available| 10_000_u16.saturating_sub(available)),
            available_basis_points: window.available_basis_points,
            delta_basis_points,
            resolution_basis_points: self.observed_resolution_basis_points,
            reset_at_ms: window.reset_at_ms,
            window_minutes: window.window_minutes,
            observed_at_ms: window.observed_at_ms,
            source,
        };
        let changed = self.observations.last().is_none_or(|previous| {
            previous.available_basis_points != observation.available_basis_points
                || previous.window_minutes != observation.window_minutes
                || previous.source != observation.source
                || previous
                    .reset_at_ms
                    .zip(observation.reset_at_ms)
                    .is_some_and(|(left, right)| left.abs_diff(right) > RESET_SKEW_MS)
        });
        if !changed {
            return;
        }
        self.observations.push(observation);
        if self.observations.len() > MAX_QUOTA_OBSERVATIONS {
            self.observations.remove(0);
        }
    }

    fn record_sample(&mut self, sample: IntervalSample) {
        let comparable = self.samples.clone();
        if comparable.len() >= DRIFT_BASELINE_SAMPLES && rate_is_outside_epoch(&comparable, sample)
        {
            self.drift_samples.push(sample);
            if self.drift_samples.len() > MAX_DRIFT_SAMPLES {
                self.drift_samples.remove(0);
            }
            let drift_basis_points = self
                .drift_samples
                .iter()
                .map(|sample| u64::from(sample.consumed_basis_points))
                .sum::<u64>();
            if drift_basis_points >= DRIFT_CONFIRMATION_BASIS_POINTS
                && drift_confirms_new_epoch(&comparable, &self.drift_samples)
            {
                self.samples = std::mem::take(&mut self.drift_samples);
                self.epoch = self.epoch.saturating_add(1);
            }
            return;
        }
        self.drift_samples.clear();
        self.samples.push(sample);
        if self.samples.len() > MAX_INTERVAL_SAMPLES {
            self.samples.remove(0);
        }
    }

    fn observe_resolution(&mut self, value: u16) {
        if value == 0 {
            return;
        }
        let fractional = value % 100;
        let candidate = if fractional == 0 {
            100
        } else {
            greatest_common_divisor(100, fractional)
        };
        self.observed_resolution_basis_points =
            self.observed_resolution_basis_points.min(candidate.max(1));
    }

    fn minimum_sample_basis_points(&self) -> u16 {
        self.observed_resolution_basis_points
            .saturating_mul(3)
            .max(100)
    }

    fn finish_cycle(&mut self, status: QuotaCycleStatus, completed_at_ms: u64) {
        let Some(cycle) = self.active_cycle.take() else {
            return;
        };
        let consumed_basis_points = cycle
            .start_available_basis_points
            .saturating_sub(cycle.last_available_basis_points);
        if consumed_basis_points == 0 && cycle.usage.is_empty() {
            return;
        }
        let record = QuotaCycleRecord {
            status,
            provider: cycle.provider,
            plan: cycle.plan,
            window_kind: cycle.window_kind.unwrap_or(QuotaWindowKind::Primary),
            window_minutes: cycle.window_minutes,
            fingerprint: cycle.fingerprint,
            started_at_ms: cycle.started_at_ms,
            completed_at_ms: completed_at_ms.max(cycle.last_observed_at_ms),
            reset_at_ms: cycle.reset_at_ms,
            pricing_revision: cycle.pricing_revision,
            epoch: cycle.epoch,
            service_tier: cycle.usage.single_service_tier(),
            standard_observations: cycle.usage.standard_observations,
            fast_observations: cycle.usage.fast_observations,
            active_observations: cycle.active_observations,
            passive_observations: cycle.passive_observations,
            consumed_basis_points,
            unattributed_basis_points: cycle.unattributed_basis_points,
            api_equivalent_micro_usd: cycle.usage.totals.api_equivalent_micro_usd,
            requests: cycle.usage.totals.requests,
            input_tokens: cycle.usage.totals.input_tokens,
            cached_input_tokens: cycle.usage.totals.cached_input_tokens,
            reasoning_tokens: cycle.usage.totals.reasoning_tokens,
            output_tokens: cycle.usage.totals.output_tokens,
            total_tokens: cycle.usage.totals.total_tokens,
        };
        if self
            .cycles
            .last()
            .is_none_or(|existing| existing.fingerprint != record.fingerprint)
        {
            self.cycles.push(record);
            if self.cycles.len() > MAX_CYCLE_RECORDS {
                self.cycles.remove(0);
            }
        }
    }
}

impl ActiveQuotaCycle {
    fn record_observation(&mut self, source: QuotaObservationSource) {
        match source {
            QuotaObservationSource::Active => {
                self.active_observations = self.active_observations.saturating_add(1)
            }
            QuotaObservationSource::Passive => {
                self.passive_observations = self.passive_observations.saturating_add(1)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingUsage {
    totals: CapacityUsage,
    #[serde(alias = "standardRequests")]
    standard_observations: u64,
    #[serde(alias = "fastRequests")]
    fast_observations: u64,
    #[serde(default)]
    first_observed_at_ms: u64,
    #[serde(default)]
    last_observed_at_ms: u64,
}

impl PendingUsage {
    fn add(&mut self, usage: QuotaEconomicsUsage, observed_at_ms: u64) {
        if self.first_observed_at_ms == 0 {
            self.first_observed_at_ms = observed_at_ms;
        }
        self.last_observed_at_ms = self.last_observed_at_ms.max(observed_at_ms);
        self.totals.add(usage);
        match usage.service_tier {
            DefaultServiceTier::Standard => {
                self.standard_observations = self.standard_observations.saturating_add(1)
            }
            DefaultServiceTier::Fast => {
                self.fast_observations = self.fast_observations.saturating_add(1)
            }
        }
    }

    fn is_empty(self) -> bool {
        self.totals.requests == 0 && self.totals.total_tokens == 0
    }

    fn single_service_tier(self) -> Option<DefaultServiceTier> {
        match (self.standard_observations > 0, self.fast_observations > 0) {
            (true, false) => Some(DefaultServiceTier::Standard),
            (false, true) => Some(DefaultServiceTier::Fast),
            _ => None,
        }
    }

    fn merge(&mut self, other: Self) {
        self.totals.merge(other.totals);
        self.standard_observations = self
            .standard_observations
            .saturating_add(other.standard_observations);
        self.fast_observations = self
            .fast_observations
            .saturating_add(other.fast_observations);
        self.first_observed_at_ms = match (self.first_observed_at_ms, other.first_observed_at_ms) {
            (0, value) | (value, 0) => value,
            (left, right) => left.min(right),
        };
        self.last_observed_at_ms = self.last_observed_at_ms.max(other.last_observed_at_ms);
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingCalibration {
    usage: PendingUsage,
    consumed_basis_points: u16,
    #[serde(default)]
    started_at_ms: u64,
    #[serde(default)]
    completed_at_ms: u64,
    #[serde(default)]
    source: QuotaObservationSource,
}

impl PendingCalibration {
    fn add(
        &mut self,
        usage: PendingUsage,
        consumed_basis_points: u16,
        source: QuotaObservationSource,
        completed_at_ms: u64,
    ) {
        let first = self.consumed_basis_points == 0;
        if self.started_at_ms == 0 {
            self.started_at_ms = usage.first_observed_at_ms;
        }
        self.usage.merge(usage);
        self.consumed_basis_points = self
            .consumed_basis_points
            .saturating_add(consumed_basis_points);
        self.completed_at_ms = completed_at_ms;
        if first || source == QuotaObservationSource::Active {
            self.source = source;
        }
    }

    fn take_sample(&mut self) -> IntervalSample {
        let value = std::mem::take(self);
        IntervalSample {
            usage: value.usage.totals,
            consumed_basis_points: value.consumed_basis_points,
            service_tier: value.usage.single_service_tier(),
            started_at_ms: value.started_at_ms,
            completed_at_ms: value.completed_at_ms,
            source: value.source,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapacityUsage {
    api_equivalent_micro_usd: Option<u64>,
    requests: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    reasoning_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

impl CapacityUsage {
    fn add(&mut self, usage: QuotaEconomicsUsage) {
        let was_empty = self.requests == 0 && self.total_tokens == 0;
        self.api_equivalent_micro_usd = match (
            was_empty,
            self.api_equivalent_micro_usd,
            usage.api_equivalent_micro_usd,
        ) {
            (true, _, value) => value,
            (false, Some(current), Some(value)) => Some(current.saturating_add(value)),
            _ => None,
        };
        self.requests = self.requests.saturating_add(usage.requests);
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(usage.cached_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(usage.reasoning_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(usage.total_tokens);
    }

    fn merge(&mut self, other: Self) {
        let was_empty = self.requests == 0 && self.total_tokens == 0;
        self.api_equivalent_micro_usd = merge_optional_total(
            was_empty,
            self.api_equivalent_micro_usd,
            other.api_equivalent_micro_usd,
        );
        self.requests = self.requests.saturating_add(other.requests);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

fn merge_optional_total(was_empty: bool, current: Option<u64>, next: Option<u64>) -> Option<u64> {
    match (was_empty, current, next) {
        (true, _, value) => value,
        (false, Some(current), Some(value)) => Some(current.saturating_add(value)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IntervalSample {
    usage: CapacityUsage,
    consumed_basis_points: u16,
    service_tier: Option<DefaultServiceTier>,
    #[serde(default)]
    started_at_ms: u64,
    #[serde(default)]
    completed_at_ms: u64,
    #[serde(default)]
    source: QuotaObservationSource,
}

fn rate_is_outside_epoch(samples: &[IntervalSample], candidate: IntervalSample) -> bool {
    if samples.len() < DRIFT_BASELINE_SAMPLES {
        return false;
    }
    weighted_calibration_rate(samples)
        .zip(primary_calibration_rate(candidate))
        .is_some_and(|(baseline, candidate)| materially_different(baseline, candidate))
}

fn drift_confirms_new_epoch(
    baseline_samples: &[IntervalSample],
    drift_samples: &[IntervalSample],
) -> bool {
    weighted_calibration_rate(baseline_samples)
        .zip(weighted_calibration_rate(drift_samples))
        .is_some_and(|(baseline, drift)| materially_different(baseline, drift))
}

/// Cost of a full window, extrapolated from one measured interval.
///
/// This is the calibration unit: measured reference dollars per unit of
/// measured quota movement. Drift detection compares accounts and epochs in
/// this unit, so it must never be derived through a second conversion.
fn primary_calibration_rate(sample: IntervalSample) -> Option<u64> {
    sample
        .usage
        .api_equivalent_micro_usd
        .map(|value| scale(value, 10_000, sample.consumed_basis_points))
}

fn weighted_calibration_rate(samples: &[IntervalSample]) -> Option<u64> {
    weighted_quantile(
        samples
            .iter()
            .filter_map(|sample| {
                Some((
                    primary_calibration_rate(*sample)?,
                    u64::from(sample.consumed_basis_points),
                ))
            })
            .collect(),
        1,
        2,
    )
}

fn materially_different(baseline: u64, candidate: u64) -> bool {
    if baseline == 0 {
        return candidate > 0;
    }
    u128::from(baseline.abs_diff(candidate)).saturating_mul(10_000)
        > u128::from(baseline).saturating_mul(u128::from(DRIFT_THRESHOLD_BASIS_POINTS))
}

fn greatest_common_divisor(mut left: u16, mut right: u16) -> u16 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaEconomicsEstimateState {
    #[default]
    Collecting,
    Estimated,
    Stale,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaEconomicsConfidence {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsSummary {
    pub purchase_cost_micro_usd: Option<u64>,
    pub potential_micro_usd: Option<u64>,
    pub potential_low_micro_usd: Option<u64>,
    pub potential_high_micro_usd: Option<u64>,
    pub potential_requests: Option<u64>,
    pub potential_total_tokens: Option<u64>,
    pub available_now_micro_usd: Option<u64>,
    pub estimate_state: QuotaEconomicsEstimateState,
    pub confidence: Option<QuotaEconomicsConfidence>,
    pub observed_basis_points: u64,
    pub sample_count: usize,
    #[serde(default)]
    pub windows: Vec<QuotaEconomicsWindowSummary>,
    #[serde(default)]
    pub cycles: Vec<QuotaCycleRecord>,
    #[serde(default)]
    pub observations: Vec<QuotaObservationRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsWindowSummary {
    pub kind: QuotaWindowKind,
    pub potential_micro_usd: Option<u64>,
    pub potential_low_micro_usd: Option<u64>,
    pub potential_high_micro_usd: Option<u64>,
    pub potential_requests: Option<u64>,
    pub potential_total_tokens: Option<u64>,
    pub full_window_micro_usd: Option<u64>,
    pub full_window_low_micro_usd: Option<u64>,
    pub full_window_high_micro_usd: Option<u64>,
    pub full_window_requests: Option<u64>,
    pub full_window_total_tokens: Option<u64>,
    pub estimate_state: QuotaEconomicsEstimateState,
    pub confidence: Option<QuotaEconomicsConfidence>,
    pub observed_basis_points: u64,
    pub sample_count: usize,
    #[serde(default)]
    pub service_tiers: Vec<QuotaEconomicsTierSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_benchmark: Option<QuotaPlanBenchmark>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsTierSummary {
    pub service_tier: DefaultServiceTier,
    pub potential_micro_usd: Option<u64>,
    pub potential_requests: Option<u64>,
    pub potential_total_tokens: Option<u64>,
    pub observed_basis_points: u64,
    pub sample_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPlanBenchmark {
    pub provider: String,
    pub plan: String,
    pub window_kind: QuotaWindowKind,
    pub window_minutes: u32,
    pub service_tier: DefaultServiceTier,
    pub pricing_revision: String,
    pub account_count: usize,
    pub cycle_count: usize,
    pub latest_completed_at_ms: u64,
    pub stale: bool,
    pub confidence: QuotaEconomicsConfidence,
    pub full_window_micro_usd: u64,
    pub mean_full_window_micro_usd: u64,
    pub low_full_window_micro_usd: u64,
    pub high_full_window_micro_usd: u64,
    pub potential_micro_usd: Option<u64>,
    pub weekly_equivalent_micro_usd: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BenchmarkKey {
    provider: String,
    plan: String,
    window_kind: QuotaWindowKind,
    window_minutes: u32,
    service_tier: String,
    pricing_revision: String,
}

const BENCHMARK_MAX_AGE_MS: u64 = 90 * 24 * 60 * 60 * 1_000;
const BENCHMARK_STALE_AFTER_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

pub fn quota_plan_benchmarks<'a>(
    accounts: impl IntoIterator<Item = (&'a str, &'a QuotaEconomicsState)>,
    now_ms: u64,
    pricing_revision: &str,
) -> Vec<QuotaPlanBenchmark> {
    let pricing_revision = pricing_revision.trim();
    let mut grouped = BTreeMap::<BenchmarkKey, BTreeMap<String, Vec<&QuotaCycleRecord>>>::new();
    for (account_id, state) in accounts {
        for history in [&state.primary, &state.secondary] {
            for cycle in &history.cycles {
                let Some(service_tier) = cycle.service_tier else {
                    continue;
                };
                let Some(plan) = cycle.plan.as_ref().filter(|value| !value.is_empty()) else {
                    continue;
                };
                let Some(window_minutes) = cycle.window_minutes.filter(|value| *value > 0) else {
                    continue;
                };
                if cycle.status != QuotaCycleStatus::Complete
                    || cycle.epoch != history.epoch
                    || cycle.unattributed_basis_points > 0
                    || cycle.consumed_basis_points < 9_900
                    || cycle.api_equivalent_micro_usd.is_none()
                    || cycle.completed_at_ms.saturating_add(BENCHMARK_MAX_AGE_MS) < now_ms
                    || cycle.pricing_revision.as_deref() != Some(pricing_revision)
                {
                    continue;
                }
                let key = BenchmarkKey {
                    provider: cycle.provider.clone(),
                    plan: plan.clone(),
                    window_kind: cycle.window_kind,
                    window_minutes,
                    service_tier: service_tier_name(service_tier).to_string(),
                    pricing_revision: pricing_revision.to_string(),
                };
                grouped
                    .entry(key)
                    .or_default()
                    .entry(account_id.to_string())
                    .or_default()
                    .push(cycle);
            }
        }
    }
    grouped
        .into_iter()
        .filter_map(|(key, mut account_cycles)| {
            let mut account_api_values = Vec::new();
            let mut cycle_count = 0_usize;
            let mut latest_completed_at_ms = 0_u64;
            for cycles in account_cycles.values_mut() {
                cycles.sort_unstable_by_key(|cycle| std::cmp::Reverse(cycle.completed_at_ms));
                cycles.truncate(3);
                cycle_count = cycle_count.saturating_add(cycles.len());
                latest_completed_at_ms = latest_completed_at_ms.max(
                    cycles
                        .iter()
                        .map(|cycle| cycle.completed_at_ms)
                        .max()
                        .unwrap_or_default(),
                );
                let mut api_values = cycles
                    .iter()
                    .filter_map(|cycle| cycle.api_equivalent_micro_usd)
                    .collect::<Vec<_>>();
                api_values.sort_unstable();
                if !api_values.is_empty() {
                    account_api_values.push(median(&api_values));
                }
            }
            if account_api_values.len() < 2 {
                return None;
            }
            account_api_values.sort_unstable();
            let full_window_micro_usd = median(&account_api_values);
            let low_full_window_micro_usd = unweighted_quantile(&account_api_values, 1, 4)?;
            let high_full_window_micro_usd = unweighted_quantile(&account_api_values, 3, 4)?;
            let dispersion = if full_window_micro_usd == 0 {
                u64::MAX
            } else {
                u64::try_from(
                    u128::from(high_full_window_micro_usd - low_full_window_micro_usd)
                        .saturating_mul(10_000)
                        / u128::from(full_window_micro_usd),
                )
                .unwrap_or(u64::MAX)
            };
            let confidence =
                if account_api_values.len() >= 10 && cycle_count >= 20 && dispersion <= 1_500 {
                    QuotaEconomicsConfidence::High
                } else if account_api_values.len() >= 5 && cycle_count >= 10 && dispersion <= 2_500
                {
                    QuotaEconomicsConfidence::Medium
                } else {
                    QuotaEconomicsConfidence::Low
                };
            let mean_full_window_micro_usd = u64::try_from(
                account_api_values
                    .iter()
                    .map(|value| u128::from(*value))
                    .sum::<u128>()
                    / account_api_values.len() as u128,
            )
            .unwrap_or(u64::MAX);
            Some(QuotaPlanBenchmark {
                provider: key.provider,
                plan: key.plan,
                window_kind: key.window_kind,
                window_minutes: key.window_minutes,
                service_tier: parse_service_tier(&key.service_tier),
                pricing_revision: key.pricing_revision,
                account_count: account_api_values.len(),
                cycle_count,
                latest_completed_at_ms,
                stale: latest_completed_at_ms.saturating_add(BENCHMARK_STALE_AFTER_MS) < now_ms,
                confidence,
                full_window_micro_usd,
                mean_full_window_micro_usd,
                low_full_window_micro_usd,
                high_full_window_micro_usd,
                potential_micro_usd: None,
                weekly_equivalent_micro_usd: (key.window_minutes >= 1_440
                    && key.window_minutes != 10_080)
                    .then(|| scale_duration(full_window_micro_usd, 10_080, key.window_minutes)),
            })
        })
        .collect()
}

pub fn attach_quota_plan_benchmarks(
    summary: &mut QuotaEconomicsSummary,
    provider: &str,
    plan: Option<&str>,
    quota: &QuotaSnapshot,
    service_tier: DefaultServiceTier,
    pricing_revision: &str,
    benchmarks: &[QuotaPlanBenchmark],
) {
    let provider = provider.trim().to_ascii_lowercase();
    let plan = plan.map(super::windows::normalize_subscription_plan);
    for window_summary in &mut summary.windows {
        let Some(window) = quota.window(window_summary.kind) else {
            continue;
        };
        let Some(window_minutes) = window.window_minutes else {
            continue;
        };
        let Some(available) = window.available_basis_points else {
            continue;
        };
        let Some(mut benchmark) = benchmarks
            .iter()
            .find(|benchmark| {
                benchmark.provider == provider
                    && Some(benchmark.plan.as_str()) == plan.as_deref()
                    && benchmark.window_kind == window_summary.kind
                    && benchmark.window_minutes == window_minutes
                    && benchmark.service_tier == service_tier
                    && benchmark.pricing_revision == pricing_revision.trim()
            })
            .cloned()
        else {
            continue;
        };
        benchmark.potential_micro_usd =
            Some(scale(benchmark.full_window_micro_usd, available, 10_000));
        window_summary.plan_benchmark = Some(benchmark);
    }
}

pub fn quota_economics_summary(
    state: &QuotaEconomicsState,
    quota: &QuotaSnapshot,
    active_service_tier: DefaultServiceTier,
    now_ms: u64,
    stale_after_ms: u64,
) -> QuotaEconomicsSummary {
    quota_economics_summary_internal(
        state,
        quota,
        active_service_tier,
        now_ms,
        stale_after_ms,
        true,
    )
}

/// Builds a summary only when the persisted calibration belongs to the
/// caller's current valuation formula. This keeps provider pricing policy out
/// of the generic quota module.
pub fn quota_economics_summary_for_revision(
    state: &QuotaEconomicsState,
    quota: &QuotaSnapshot,
    active_service_tier: DefaultServiceTier,
    now_ms: u64,
    stale_after_ms: u64,
    revision: &str,
) -> QuotaEconomicsSummary {
    quota_economics_summary_internal(
        state,
        quota,
        active_service_tier,
        now_ms,
        stale_after_ms,
        state.pricing_revision.as_deref() == Some(revision.trim()),
    )
}

fn quota_economics_summary_internal(
    state: &QuotaEconomicsState,
    quota: &QuotaSnapshot,
    _active_service_tier: DefaultServiceTier,
    now_ms: u64,
    stale_after_ms: u64,
    use_history: bool,
) -> QuotaEconomicsSummary {
    let empty_primary = WindowEconomicsHistory::default();
    let empty_secondary = WindowEconomicsHistory::default();
    let primary_history = if use_history {
        &state.primary
    } else {
        &empty_primary
    };
    let secondary_history = if use_history {
        &state.secondary
    } else {
        &empty_secondary
    };
    let primary = window_summary(
        QuotaWindowKind::Primary,
        primary_history,
        quota.primary.as_ref(),
        now_ms,
        stale_after_ms,
    );
    let secondary = window_summary(
        QuotaWindowKind::Secondary,
        secondary_history,
        quota.secondary.as_ref(),
        now_ms,
        stale_after_ms,
    );
    let stock = stock_estimate(
        &primary,
        &secondary,
        quota.primary.as_ref(),
        quota.secondary.as_ref(),
    );
    let available_now = limiting_estimate(&primary, &secondary);
    let selected = stock
        .source
        .unwrap_or_else(|| select_summary(&primary, &secondary));
    QuotaEconomicsSummary {
        purchase_cost_micro_usd: state.purchase_cost_micro_usd,
        potential_micro_usd: stock.potential.median,
        potential_low_micro_usd: stock.potential.low,
        potential_high_micro_usd: stock.potential.high,
        potential_requests: stock.requests,
        potential_total_tokens: stock.total_tokens,
        available_now_micro_usd: available_now.potential.median,
        estimate_state: selected.estimate_state,
        confidence: selected.confidence,
        observed_basis_points: selected.observed_basis_points,
        sample_count: selected.sample_count,
        windows: vec![primary, secondary],
        cycles: recent_cycles(state),
        observations: recent_observations(state),
    }
}

fn recent_observations(state: &QuotaEconomicsState) -> Vec<QuotaObservationRecord> {
    let mut observations = state
        .primary
        .observations
        .iter()
        .chain(&state.secondary.observations)
        .cloned()
        .collect::<Vec<_>>();
    observations.sort_unstable_by_key(|observation| std::cmp::Reverse(observation.observed_at_ms));
    observations.truncate(MAX_QUOTA_OBSERVATIONS);
    observations
}

fn recent_cycles(state: &QuotaEconomicsState) -> Vec<QuotaCycleRecord> {
    let mut cycles = state
        .primary
        .cycles
        .iter()
        .chain(&state.secondary.cycles)
        .cloned()
        .collect::<Vec<_>>();
    cycles.sort_unstable_by_key(|cycle| std::cmp::Reverse(cycle.completed_at_ms));
    cycles.truncate(MAX_CYCLE_RECORDS);
    cycles
}

fn window_summary(
    kind: QuotaWindowKind,
    history: &WindowEconomicsHistory,
    window: Option<&QuotaWindow>,
    now_ms: u64,
    stale_after_ms: u64,
) -> QuotaEconomicsWindowSummary {
    let samples = history
        .samples
        .iter()
        .copied()
        .filter(|sample| primary_calibration_rate(*sample).is_some())
        .collect::<Vec<_>>();
    let observed_basis_points = samples
        .iter()
        .map(|sample| u64::from(sample.consumed_basis_points))
        .sum();
    let sample_count = samples.len();
    if observed_basis_points < MIN_ESTIMATE_BASIS_POINTS {
        let mut summary = empty_window_summary(
            kind,
            QuotaEconomicsEstimateState::Collecting,
            observed_basis_points,
            sample_count,
        );
        if let Some(available) =
            window.and_then(|window| fresh_available(window, now_ms, stale_after_ms))
        {
            summary.service_tiers = service_tier_summaries(history, available);
        }
        return summary;
    }
    let Some(available) = window.and_then(|window| fresh_available(window, now_ms, stale_after_ms))
    else {
        return empty_window_summary(
            kind,
            QuotaEconomicsEstimateState::Stale,
            observed_basis_points,
            sample_count,
        );
    };
    // Every published figure is the same estimator applied to a directly
    // measured quantity. Requests and tokens are not projected from the money
    // figure, and the money figure is not projected from anything: a single
    // conversion step here would make the band describe the dispersion of one
    // metric while the median describes another.
    let potential = metric_estimate(&samples, available, |usage| usage.api_equivalent_micro_usd);
    let requests = metric_estimate(&samples, available, |usage| Some(usage.requests));
    let tokens = metric_estimate(&samples, available, |usage| Some(usage.total_tokens));
    let full = metric_estimate(&samples, 10_000, |usage| usage.api_equivalent_micro_usd);
    let full_requests = metric_estimate(&samples, 10_000, |usage| Some(usage.requests));
    let full_tokens = metric_estimate(&samples, 10_000, |usage| Some(usage.total_tokens));
    let has_complete_cycle = history.cycles.iter().any(|cycle| {
        cycle.epoch == history.epoch
            && cycle.status == QuotaCycleStatus::Complete
            && cycle.unattributed_basis_points == 0
            && cycle.consumed_basis_points >= 9_900
            && cycle.api_equivalent_micro_usd.is_some()
    });
    QuotaEconomicsWindowSummary {
        kind,
        potential_micro_usd: potential.median,
        potential_low_micro_usd: potential.low,
        potential_high_micro_usd: potential.high,
        potential_requests: requests.median,
        potential_total_tokens: tokens.median,
        full_window_micro_usd: full.median,
        full_window_low_micro_usd: full.low,
        full_window_high_micro_usd: full.high,
        full_window_requests: full_requests.median,
        full_window_total_tokens: full_tokens.median,
        estimate_state: QuotaEconomicsEstimateState::Estimated,
        confidence: Some(confidence(
            observed_basis_points,
            &samples,
            has_complete_cycle,
        )),
        observed_basis_points,
        sample_count,
        service_tiers: service_tier_summaries(history, available),
        plan_benchmark: None,
    }
}

fn service_tier_summaries(
    history: &WindowEconomicsHistory,
    available: u16,
) -> Vec<QuotaEconomicsTierSummary> {
    [DefaultServiceTier::Standard, DefaultServiceTier::Fast]
        .into_iter()
        .filter_map(|tier| tier_summary(history, tier, available))
        .collect()
}

fn tier_summary(
    history: &WindowEconomicsHistory,
    tier: DefaultServiceTier,
    available: u16,
) -> Option<QuotaEconomicsTierSummary> {
    // The presence filter has to be repeated here: `observed_basis_points`
    // below decides whether the tier is published at all, so counting a sample
    // this estimator cannot value would advertise a tier with an empty figure.
    let samples = history
        .samples
        .iter()
        .copied()
        .filter(|sample| {
            sample.service_tier == Some(tier) && primary_calibration_rate(*sample).is_some()
        })
        .collect::<Vec<_>>();
    let observed_basis_points = samples
        .iter()
        .map(|sample| u64::from(sample.consumed_basis_points))
        .sum::<u64>();
    (observed_basis_points >= MIN_ESTIMATE_BASIS_POINTS).then(|| QuotaEconomicsTierSummary {
        service_tier: tier,
        potential_micro_usd: metric_estimate(&samples, available, |usage| {
            usage.api_equivalent_micro_usd
        })
        .median,
        potential_requests: metric_estimate(&samples, available, |usage| Some(usage.requests))
            .median,
        potential_total_tokens: metric_estimate(&samples, available, |usage| {
            Some(usage.total_tokens)
        })
        .median,
        observed_basis_points,
        sample_count: samples.len(),
    })
}

#[derive(Clone, Copy, Default)]
struct MetricEstimate {
    median: Option<u64>,
    low: Option<u64>,
    high: Option<u64>,
}

fn metric_estimate(
    samples: &[IntervalSample],
    available_basis_points: u16,
    value: impl Fn(CapacityUsage) -> Option<u64>,
) -> MetricEstimate {
    let estimates = samples
        .iter()
        .filter_map(|sample| {
            let measured = value(sample.usage)?;
            (measured > 0).then_some((
                scale(
                    measured,
                    available_basis_points,
                    sample.consumed_basis_points,
                ),
                u64::from(sample.consumed_basis_points),
            ))
        })
        .collect::<Vec<_>>();
    if estimates.is_empty() {
        return MetricEstimate::default();
    }
    MetricEstimate {
        median: weighted_quantile(estimates.clone(), 1, 2),
        low: weighted_quantile(estimates.clone(), 1, 4),
        high: weighted_quantile(estimates, 3, 4),
    }
}

fn select_summary<'a>(
    primary: &'a QuotaEconomicsWindowSummary,
    secondary: &'a QuotaEconomicsWindowSummary,
) -> &'a QuotaEconomicsWindowSummary {
    if secondary.observed_basis_points > primary.observed_basis_points
        || (secondary.observed_basis_points == primary.observed_basis_points
            && secondary.sample_count >= primary.sample_count)
    {
        secondary
    } else {
        primary
    }
}

#[derive(Default)]
struct CombinedEstimate<'a> {
    potential: MetricEstimate,
    requests: Option<u64>,
    total_tokens: Option<u64>,
    source: Option<&'a QuotaEconomicsWindowSummary>,
}

fn stock_estimate<'a>(
    primary: &'a QuotaEconomicsWindowSummary,
    secondary: &'a QuotaEconomicsWindowSummary,
    primary_window: Option<&QuotaWindow>,
    secondary_window: Option<&QuotaWindow>,
) -> CombinedEstimate<'a> {
    [(primary, primary_window), (secondary, secondary_window)]
        .into_iter()
        .filter(|(summary, _)| window_has_estimate(summary))
        .max_by_key(|(summary, window)| {
            (
                window
                    .and_then(|window| window.window_minutes)
                    .unwrap_or_default(),
                summary.observed_basis_points,
                summary.sample_count,
            )
        })
        .map_or_else(CombinedEstimate::default, |(summary, _)| {
            estimate_from_window(summary)
        })
}

fn limiting_estimate<'a>(
    primary: &'a QuotaEconomicsWindowSummary,
    secondary: &'a QuotaEconomicsWindowSummary,
) -> CombinedEstimate<'a> {
    match primary_is_limiting(primary, secondary) {
        Some(true) => estimate_from_window(primary),
        Some(false) => estimate_from_window(secondary),
        None => CombinedEstimate::default(),
    }
}

fn primary_is_limiting(
    primary: &QuotaEconomicsWindowSummary,
    secondary: &QuotaEconomicsWindowSummary,
) -> Option<bool> {
    [
        (primary.potential_micro_usd, secondary.potential_micro_usd),
        (
            primary.potential_total_tokens,
            secondary.potential_total_tokens,
        ),
        (primary.potential_requests, secondary.potential_requests),
    ]
    .into_iter()
    .find_map(|(primary, secondary)| {
        primary
            .zip(secondary)
            .map(|(primary, secondary)| primary <= secondary)
    })
    .or_else(|| {
        let primary_has_estimate = window_has_estimate(primary);
        let secondary_has_estimate = window_has_estimate(secondary);
        match (primary_has_estimate, secondary_has_estimate) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            (true, true) => Some(select_summary(primary, secondary).kind == primary.kind),
            (false, false) => None,
        }
    })
}

fn window_has_estimate(window: &QuotaEconomicsWindowSummary) -> bool {
    window.potential_micro_usd.is_some()
        || window.potential_total_tokens.is_some()
        || window.potential_requests.is_some()
}

fn estimate_from_window(window: &QuotaEconomicsWindowSummary) -> CombinedEstimate<'_> {
    CombinedEstimate {
        potential: MetricEstimate {
            median: window.potential_micro_usd,
            low: window.potential_low_micro_usd,
            high: window.potential_high_micro_usd,
        },
        requests: window.potential_requests,
        total_tokens: window.potential_total_tokens,
        source: Some(window),
    }
}

fn confidence(
    observed_basis_points: u64,
    samples: &[IntervalSample],
    has_complete_cycle: bool,
) -> QuotaEconomicsConfidence {
    let sample_count = samples.len();
    let dispersion_bps = rate_dispersion_bps(samples);
    if has_complete_cycle {
        QuotaEconomicsConfidence::High
    } else if observed_basis_points >= MEDIUM_CONFIDENCE_OBSERVED_BASIS_POINTS
        && sample_count >= 3
        && dispersion_bps.is_some_and(|value| value <= MEDIUM_CONFIDENCE_IQR_BPS)
    {
        QuotaEconomicsConfidence::Medium
    } else {
        QuotaEconomicsConfidence::Low
    }
}

fn rate_dispersion_bps(samples: &[IntervalSample]) -> Option<u64> {
    let rates = samples
        .iter()
        .filter_map(|sample| {
            Some((
                primary_calibration_rate(*sample)?,
                u64::from(sample.consumed_basis_points),
            ))
        })
        .collect::<Vec<_>>();
    if rates.len() < 2 {
        return None;
    }
    let middle = weighted_quantile(rates.clone(), 1, 2)?;
    if middle == 0 {
        return None;
    }
    let low = weighted_quantile(rates.clone(), 1, 4)?;
    let high = weighted_quantile(rates, 3, 4)?;
    Some(
        u64::try_from(u128::from(high.saturating_sub(low)) * 10_000 / u128::from(middle))
            .unwrap_or(u64::MAX),
    )
}

fn scale(measured: u64, available_basis_points: u16, consumed_basis_points: u16) -> u64 {
    u64::try_from(
        u128::from(measured) * u128::from(available_basis_points)
            / u128::from(consumed_basis_points),
    )
    .unwrap_or(u64::MAX)
}

fn median(values: &[u64]) -> u64 {
    if values.len().is_multiple_of(2) {
        values[values.len() / 2 - 1].saturating_add(values[values.len() / 2]) / 2
    } else {
        values[values.len() / 2]
    }
}

fn unweighted_quantile(values: &[u64], numerator: usize, denominator: usize) -> Option<u64> {
    if values.is_empty() || denominator == 0 || numerator > denominator {
        return None;
    }
    let index = values
        .len()
        .saturating_mul(numerator)
        .div_ceil(denominator)
        .saturating_sub(1);
    values.get(index).copied()
}

fn service_tier_name(tier: DefaultServiceTier) -> &'static str {
    match tier {
        DefaultServiceTier::Standard => "standard",
        DefaultServiceTier::Fast => "fast",
    }
}

fn parse_service_tier(value: &str) -> DefaultServiceTier {
    if value == "fast" {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

fn scale_duration(value: u64, target_minutes: u32, source_minutes: u32) -> u64 {
    u64::try_from(
        u128::from(value).saturating_mul(u128::from(target_minutes))
            / u128::from(source_minutes.max(1)),
    )
    .unwrap_or(u64::MAX)
}

fn weighted_quantile(mut values: Vec<(u64, u64)>, numerator: u64, denominator: u64) -> Option<u64> {
    if values.is_empty() || denominator == 0 || numerator > denominator {
        return None;
    }
    values.sort_unstable_by_key(|(value, _)| *value);
    let total_weight = values.iter().map(|(_, weight)| *weight).sum::<u64>();
    if total_weight == 0 {
        return None;
    }
    let target = u128::from(total_weight)
        .saturating_mul(u128::from(numerator))
        .div_ceil(u128::from(denominator));
    let mut cumulative = 0_u128;
    values.into_iter().find_map(|(value, weight)| {
        cumulative = cumulative.saturating_add(u128::from(weight));
        (cumulative >= target).then_some(value)
    })
}

fn fresh_available(window: &QuotaWindow, now_ms: u64, stale_after_ms: u64) -> Option<u16> {
    (window.observed_at_ms.saturating_add(stale_after_ms) >= now_ms)
        .then_some(window.available_basis_points)
        .flatten()
}

fn empty_window_summary(
    kind: QuotaWindowKind,
    estimate_state: QuotaEconomicsEstimateState,
    observed_basis_points: u64,
    sample_count: usize,
) -> QuotaEconomicsWindowSummary {
    QuotaEconomicsWindowSummary {
        kind,
        potential_micro_usd: None,
        potential_low_micro_usd: None,
        potential_high_micro_usd: None,
        potential_requests: None,
        potential_total_tokens: None,
        full_window_micro_usd: None,
        full_window_low_micro_usd: None,
        full_window_high_micro_usd: None,
        full_window_requests: None,
        full_window_total_tokens: None,
        estimate_state,
        confidence: None,
        observed_basis_points,
        sample_count,
        service_tiers: Vec::new(),
        plan_benchmark: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApiEquivalentSummary;

    #[test]
    fn estimates_after_fractional_live_quota_movement() {
        let mut state = QuotaEconomicsState {
            purchase_cost_micro_usd: Some(20_000_000),
            ..Default::default()
        };
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        for step in 1_u16..=10 {
            state.observe_usage(
                usage(500_000, 5, 50_000, DefaultServiceTier::Standard),
                None,
            );
            state.observe_quota(&snapshot(
                10_000 - step * 10,
                30_000,
                1_000 + u64::from(step),
            ));
        }

        let summary = quota_economics_summary(
            &state,
            &snapshot(9_900, 30_000, 2_000),
            DefaultServiceTier::Standard,
            2_000,
            1_000,
        );

        assert_eq!(summary.potential_micro_usd, Some(495_000_000));
        assert_eq!(summary.potential_requests, Some(4_950));
        assert_eq!(summary.potential_total_tokens, Some(49_500_000));
        assert_eq!(summary.observed_basis_points, 100);
        assert_eq!(summary.sample_count, 1);
        assert_eq!(summary.confidence, Some(QuotaEconomicsConfidence::Low));
    }

    #[test]
    fn quota_drop_without_zenith_usage_is_not_learned() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_quota(&snapshot(9_000, 30_000, 2_000));

        let summary = quota_economics_summary(
            &state,
            &snapshot(9_000, 30_000, 2_000),
            DefaultServiceTier::Standard,
            2_100,
            1_000,
        );

        assert_eq!(
            summary.estimate_state,
            QuotaEconomicsEstimateState::Collecting
        );
        assert_eq!(summary.sample_count, 0);
    }

    #[test]
    fn primary_secondary_and_service_tiers_learn_independently() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&two_window_snapshot(10_000, 10_000, 1_000));
        state.observe_usage(
            usage(200_000, 2, 20_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&two_window_snapshot(9_700, 9_400, 2_000));
        state.observe_usage(usage(400_000, 4, 40_000, DefaultServiceTier::Fast), None);
        state.observe_quota(&two_window_snapshot(9_100, 9_100, 3_000));

        let summary = quota_economics_summary(
            &state,
            &two_window_snapshot(9_100, 9_100, 3_000),
            DefaultServiceTier::Standard,
            3_100,
            1_000,
        );
        let primary = summary
            .windows
            .iter()
            .find(|window| window.kind == QuotaWindowKind::Primary)
            .unwrap();
        let secondary = summary
            .windows
            .iter()
            .find(|window| window.kind == QuotaWindowKind::Secondary)
            .unwrap();

        assert_eq!(primary.observed_basis_points, 900);
        assert_eq!(secondary.observed_basis_points, 900);
        assert_eq!(primary.service_tiers.len(), 2);
        assert_eq!(secondary.service_tiers.len(), 2);
        assert_ne!(primary.potential_micro_usd, secondary.potential_micro_usd);
    }

    #[test]
    fn normalized_personal_estimate_combines_standard_and_fast_samples() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot(9_700, 30_000, 2_000));
        state.observe_usage(usage(1_000_000, 1, 10_000, DefaultServiceTier::Fast), None);
        let quota = snapshot(9_100, 30_000, 3_000);
        state.observe_quota(&quota);

        let standard =
            quota_economics_summary(&state, &quota, DefaultServiceTier::Standard, 3_100, 1_000);
        let fast = quota_economics_summary(&state, &quota, DefaultServiceTier::Fast, 3_100, 1_000);

        assert_eq!(standard.observed_basis_points, 900);
        assert_eq!(fast.observed_basis_points, 900);
        assert_eq!(standard.potential_micro_usd, fast.potential_micro_usd);
        assert_eq!(standard.potential_requests, fast.potential_requests);
        assert_eq!(standard.potential_total_tokens, fast.potential_total_tokens);
    }

    #[test]
    fn confidence_reflects_calibration_dispersion() {
        let summary_for = |values: [u64; 3]| {
            let samples = values
                .map(|api_equivalent_micro_usd| IntervalSample {
                    usage: CapacityUsage {
                        api_equivalent_micro_usd: Some(api_equivalent_micro_usd),
                        requests: 1,
                        total_tokens: 10_000,
                        ..Default::default()
                    },
                    consumed_basis_points: 400,
                    service_tier: Some(DefaultServiceTier::Standard),
                    ..Default::default()
                })
                .to_vec();
            let state = QuotaEconomicsState {
                secondary: WindowEconomicsHistory {
                    samples,
                    ..Default::default()
                },
                ..Default::default()
            };
            quota_economics_summary(
                &state,
                &snapshot(9_000, 30_000, 2_000),
                DefaultServiceTier::Standard,
                2_100,
                1_000,
            )
            .confidence
        };

        assert_eq!(
            summary_for([1_000_000; 3]),
            Some(QuotaEconomicsConfidence::Medium)
        );
        assert_eq!(
            summary_for([1_000_000, 3_000_000, 5_000_000]),
            Some(QuotaEconomicsConfidence::Low)
        );
    }

    #[test]
    fn mixed_service_tier_interval_calibrates_only_the_normalized_total() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_usage(usage(1_000_000, 1, 10_000, DefaultServiceTier::Fast), None);
        let quota = snapshot(9_700, 30_000, 2_000);
        state.observe_quota(&quota);

        for tier in [DefaultServiceTier::Standard, DefaultServiceTier::Fast] {
            let summary = quota_economics_summary(&state, &quota, tier, 2_100, 1_000);
            assert_eq!(summary.sample_count, 1);
            assert!(summary.potential_micro_usd.is_some());
            assert!(summary.windows[1].service_tiers.is_empty());
        }
    }

    #[test]
    fn unpriced_interval_does_not_guess_capacity() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            QuotaEconomicsUsage {
                api_equivalent_micro_usd: None,
                requests: 2,
                input_tokens: 10_000,
                output_tokens: 10_000,
                total_tokens: 20_000,
                service_tier: DefaultServiceTier::Standard,
                ..Default::default()
            },
            None,
        );
        let quota = snapshot(9_700, 30_000, 2_000);
        state.observe_quota(&quota);

        let summary =
            quota_economics_summary(&state, &quota, DefaultServiceTier::Standard, 2_100, 1_000);

        assert_eq!(
            summary.estimate_state,
            QuotaEconomicsEstimateState::Collecting
        );
        assert_eq!(summary.potential_micro_usd, None);
        assert_eq!(summary.potential_requests, None);
        assert_eq!(summary.potential_total_tokens, None);
    }

    #[test]
    fn overall_estimate_survives_when_one_tier_is_still_collecting() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot(9_700, 30_000, 2_000));
        state.observe_usage(usage(1_000_000, 1, 10_000, DefaultServiceTier::Fast), None);
        let quota = snapshot(9_600, 30_000, 3_000);
        state.observe_quota(&quota);

        let summary =
            quota_economics_summary(&state, &quota, DefaultServiceTier::Fast, 3_100, 1_000);
        let secondary = summary
            .windows
            .iter()
            .find(|window| window.kind == QuotaWindowKind::Secondary)
            .unwrap();

        assert_eq!(
            summary.estimate_state,
            QuotaEconomicsEstimateState::Estimated
        );
        assert_eq!(summary.observed_basis_points, 300);
        assert!(summary.potential_micro_usd.is_some());
        assert_eq!(secondary.service_tiers.len(), 1);
        assert_eq!(
            secondary.service_tiers[0].service_tier,
            DefaultServiceTier::Standard
        );
    }

    #[test]
    fn duration_change_starts_a_new_calibration_epoch() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot_with_minutes(10_000, 30_000, 1_000, 10_080));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot_with_minutes(9_900, 30_000, 2_000, 10_080));
        state.observe_quota(&snapshot_with_minutes(9_900, 30_000, 3_000, 50_400));

        let summary = quota_economics_summary(
            &state,
            &snapshot_with_minutes(9_900, 30_000, 3_000, 50_400),
            DefaultServiceTier::Standard,
            3_100,
            1_000,
        );

        assert_eq!(
            summary.estimate_state,
            QuotaEconomicsEstimateState::Collecting
        );
        assert_eq!(summary.sample_count, 0);
    }

    #[test]
    fn pricing_revision_invalidates_persisted_samples() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        let quota = snapshot(9_700, 30_000, 2_000);
        state.observe_quota(&quota);
        assert_eq!(state.secondary.samples.len(), 1);
        state.pricing_revision = Some("obsolete-catalog".to_string());

        let revision = "current-catalog";
        let summary = quota_economics_summary_for_revision(
            &state,
            &quota,
            DefaultServiceTier::Standard,
            2_100,
            1_000,
            revision,
        );
        assert_eq!(summary.sample_count, 0);
        assert_eq!(summary.potential_micro_usd, None);

        state.set_value_revision(revision);
        assert!(state.secondary.samples.is_empty());
        assert_eq!(state.pricing_revision.as_deref(), Some(revision));
    }

    #[test]
    fn subscription_plan_change_starts_a_new_calibration_epoch() {
        let mut state = QuotaEconomicsState::default();
        configure_account(&mut state, "plus", "test-revision");
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot(9_700, 30_000, 2_000));
        let previous_epoch = state.secondary.epoch;
        assert_eq!(state.secondary.samples.len(), 1);

        state.set_account_context("chatgpt", Some("business"));

        assert_eq!(state.plan.as_deref(), Some("business"));
        assert_eq!(state.secondary.epoch, previous_epoch + 1);
        assert!(state.secondary.samples.is_empty());
        assert!(state.secondary.active_cycle.is_none());
        assert_eq!(
            quota_economics_summary(
                &state,
                &snapshot(9_700, 30_000, 2_000),
                DefaultServiceTier::Standard,
                2_100,
                1_000,
            )
            .estimate_state,
            QuotaEconomicsEstimateState::Collecting
        );
    }

    #[test]
    fn stale_quota_hides_an_existing_estimate() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot(9_700, 30_000, 2_000));

        let summary = quota_economics_summary(
            &state,
            &snapshot(9_700, 30_000, 2_000),
            DefaultServiceTier::Standard,
            10_000,
            1_000,
        );

        assert_eq!(summary.estimate_state, QuotaEconomicsEstimateState::Stale);
        assert_eq!(summary.potential_micro_usd, None);
    }

    #[test]
    fn reset_discards_unfinished_usage_from_the_previous_cycle() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(9_000, 2_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot(10_000, 30_000, 3_000));
        state.observe_quota(&snapshot(9_990, 30_000, 4_000));

        let summary = quota_economics_summary(
            &state,
            &snapshot(9_990, 30_000, 4_000),
            DefaultServiceTier::Standard,
            4_100,
            1_000,
        );

        assert_eq!(
            summary.estimate_state,
            QuotaEconomicsEstimateState::Collecting
        );
        assert_eq!(summary.sample_count, 0);
    }

    #[test]
    fn combined_capacity_is_capped_by_the_limiting_window() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&two_window_snapshot(10_000, 10_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&two_window_snapshot(9_900, 9_000, 2_000));

        let summary = quota_economics_summary(
            &state,
            &two_window_snapshot(9_900, 9_000, 2_000),
            DefaultServiceTier::Standard,
            2_100,
            1_000,
        );

        assert_eq!(summary.potential_micro_usd, Some(9_000_000));
    }

    #[test]
    fn combined_summary_uses_one_limiting_window_for_every_metric() {
        let primary_sample = IntervalSample {
            usage: CapacityUsage {
                api_equivalent_micro_usd: Some(1_000_000),
                requests: 10,
                total_tokens: 100_000,
                ..Default::default()
            },
            consumed_basis_points: 100,
            service_tier: Some(DefaultServiceTier::Standard),
            ..Default::default()
        };
        let secondary_sample = IntervalSample {
            usage: CapacityUsage {
                api_equivalent_micro_usd: Some(2_000_000),
                requests: 100,
                total_tokens: 1_000_000,
                ..Default::default()
            },
            consumed_basis_points: 100,
            service_tier: Some(DefaultServiceTier::Standard),
            ..Default::default()
        };
        let state = QuotaEconomicsState {
            pricing_revision: Some("test-catalog".to_string()),
            primary: WindowEconomicsHistory {
                samples: vec![primary_sample],
                ..Default::default()
            },
            secondary: WindowEconomicsHistory {
                samples: vec![secondary_sample, secondary_sample],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut quota = two_window_snapshot(9_000, 9_000, 2_000);
        quota.secondary.as_mut().unwrap().reset_at_ms = Some(30_000);

        let summary =
            quota_economics_summary(&state, &quota, DefaultServiceTier::Standard, 2_100, 1_000);

        assert_eq!(summary.potential_micro_usd, Some(180_000_000));
        assert_eq!(summary.available_now_micro_usd, Some(90_000_000));
        assert_eq!(summary.potential_low_micro_usd, Some(180_000_000));
        assert_eq!(summary.potential_high_micro_usd, Some(180_000_000));
        assert_eq!(summary.potential_requests, Some(9_000));
        assert_eq!(summary.potential_total_tokens, Some(90_000_000));
        assert_eq!(summary.sample_count, 2);
        assert_eq!(summary.confidence, Some(QuotaEconomicsConfidence::Low));
    }

    #[test]
    fn failed_request_with_usage_contributes_capacity_not_request_count() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_event(
            &failed_usage_event(DefaultServiceTier::Fast),
            ApiEquivalentSummary {
                micro_usd: 1_000_000,
                priced_tokens: 10_000,
                unpriced_tokens: 0,
            },
        );
        let quota = snapshot(9_700, 30_000, 2_000);
        state.observe_quota(&quota);

        let summary =
            quota_economics_summary(&state, &quota, DefaultServiceTier::Fast, 2_100, 1_000);

        assert_eq!(summary.sample_count, 1);
        // Both figures are the measured quantity extrapolated once:
        // 1_000_000 * 9_700 / 300 and 10_000 * 9_700 / 300. A second conversion
        // step anywhere in the read path would move these by a few units.
        assert_eq!(summary.potential_micro_usd, Some(32_333_333));
        assert_eq!(summary.potential_total_tokens, Some(323_333));
        assert_eq!(summary.potential_requests, None);
    }

    #[test]
    fn sustained_cost_rate_drift_starts_a_new_epoch() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        for (index, micro_usd) in [1_000, 1_000, 1_000, 10_000, 10_000]
            .into_iter()
            .enumerate()
        {
            state.observe_usage(
                usage(micro_usd, 1, 10_000, DefaultServiceTier::Standard),
                None,
            );
            let step = u16::try_from(index + 1).unwrap();
            state.observe_quota(&snapshot(
                10_000 - step * 300,
                30_000,
                2_000 + u64::from(step),
            ));
        }

        let summary = quota_economics_summary(
            &state,
            &snapshot(8_500, 30_000, 3_000),
            DefaultServiceTier::Standard,
            3_100,
            1_000,
        );

        // Only the post-drift samples survive, so the published rate is the new
        // one: 10_000 * 8_500 / 300, not a blend with the pre-drift 1_000.
        assert_eq!(summary.sample_count, 2);
        assert_eq!(summary.potential_micro_usd, Some(283_333));
    }

    #[test]
    fn integer_quota_waits_for_three_percent_before_learning() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        for step in 1_u16..=2 {
            state.observe_usage(
                usage(100_000, 1, 10_000, DefaultServiceTier::Standard),
                None,
            );
            state.observe_quota(&snapshot(
                10_000 - step * 100,
                30_000,
                1_000 + u64::from(step),
            ));
        }
        assert!(state.secondary.samples.is_empty());

        state.observe_usage(
            usage(100_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot(9_700, 30_000, 4_000));
        assert_eq!(state.secondary.samples.len(), 1);
        assert_eq!(state.secondary.samples[0].consumed_basis_points, 300);
    }

    #[test]
    fn full_cycle_is_persisted_and_external_drop_is_contaminated() {
        let mut clean = QuotaEconomicsState::default();
        configure_account(&mut clean, "plus", "test-revision");
        clean.observe_quota(&snapshot(10_000, 30_000, 1_000));
        clean.observe_usage(
            usage(4_000_000, 10, 100_000, DefaultServiceTier::Standard),
            None,
        );
        clean.observe_quota(&snapshot(0, 30_000, 2_000));
        assert_eq!(clean.secondary.cycles.len(), 1);
        assert_eq!(clean.secondary.cycles[0].status, QuotaCycleStatus::Complete);
        assert_eq!(clean.secondary.cycles[0].consumed_basis_points, 10_000);
        assert_eq!(
            quota_economics_summary(
                &clean,
                &snapshot(0, 30_000, 2_000),
                DefaultServiceTier::Standard,
                2_100,
                1_000,
            )
            .confidence,
            Some(QuotaEconomicsConfidence::High)
        );

        let encoded = serde_json::to_string(&clean).unwrap();
        let restored: QuotaEconomicsState = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.secondary.cycles, clean.secondary.cycles);

        let mut contaminated = QuotaEconomicsState::default();
        configure_account(&mut contaminated, "plus", "test-revision");
        contaminated.observe_quota(&snapshot(10_000, 30_000, 1_000));
        contaminated.observe_quota(&snapshot(9_000, 30_000, 1_500));
        contaminated.observe_usage(
            usage(4_000_000, 10, 100_000, DefaultServiceTier::Standard),
            None,
        );
        contaminated.observe_quota(&snapshot(0, 30_000, 2_000));
        assert_eq!(
            contaminated.secondary.cycles[0].status,
            QuotaCycleStatus::Contaminated
        );
        assert_eq!(
            contaminated.secondary.cycles[0].unattributed_basis_points,
            1_000
        );
    }

    #[test]
    fn limit_reached_completes_the_limiting_cycle_with_a_nonzero_reported_remainder() {
        let mut state = QuotaEconomicsState::default();
        configure_account(&mut state, "plus", "test-revision");
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(4_000_000, 10, 100_000, DefaultServiceTier::Standard),
            None,
        );
        let mut exhausted = snapshot(350, 30_000, 2_000);
        exhausted.limit_reached = true;
        state.observe_quota(&exhausted);

        assert_eq!(state.secondary.cycles.len(), 1);
        assert_eq!(state.secondary.cycles[0].status, QuotaCycleStatus::Complete);
        assert_eq!(state.secondary.cycles[0].consumed_basis_points, 10_000);
        assert!(state.secondary.active_cycle.is_none());
    }

    #[test]
    fn reset_before_exhaustion_records_a_censored_cycle() {
        let mut state = QuotaEconomicsState::default();
        configure_account(&mut state, "plus", "test-revision");
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        state.observe_usage(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
        );
        state.observe_quota(&snapshot(8_000, 30_000, 2_000));
        state.observe_quota(&snapshot(10_000, 200_000, 3_000));

        assert_eq!(state.secondary.cycles.len(), 1);
        assert_eq!(state.secondary.cycles[0].status, QuotaCycleStatus::Censored);
        assert_eq!(state.secondary.cycles[0].consumed_basis_points, 2_000);
        assert!(state.secondary.active_cycle.is_some());
    }

    #[test]
    fn plan_benchmark_is_account_first_and_rejects_mixed_cycles() {
        let mut first = QuotaEconomicsState::default();
        let mut second = QuotaEconomicsState::default();
        for (state, values) in [
            (&mut first, [100_u64, 200, 300]),
            (&mut second, [1_000_u64, 1_100, 1_200]),
        ] {
            state.secondary.cycles = values
                .into_iter()
                .enumerate()
                .map(|(index, value)| complete_cycle(value, 1_000 + index as u64))
                .collect();
            state.secondary.epoch = 1;
        }
        first.secondary.cycles.push(QuotaCycleRecord {
            service_tier: None,
            api_equivalent_micro_usd: Some(50_000),
            ..complete_cycle(50_000, 2_000)
        });
        first.secondary.cycles.push(QuotaCycleRecord {
            epoch: 0,
            ..complete_cycle(500_000, 900)
        });

        let benchmarks = quota_plan_benchmarks(
            [("first", &first), ("second", &second)],
            5_000,
            "test-revision",
        );
        assert_eq!(benchmarks.len(), 1);
        let benchmark = &benchmarks[0];
        assert_eq!(benchmark.account_count, 2);
        assert_eq!(benchmark.cycle_count, 6);
        assert_eq!(benchmark.full_window_micro_usd, 650);
        assert_eq!(benchmark.mean_full_window_micro_usd, 650);
        assert_eq!(benchmark.low_full_window_micro_usd, 200);
        assert_eq!(benchmark.high_full_window_micro_usd, 1_100);
    }

    #[test]
    fn requested_fast_attributes_the_sample_when_upstream_reports_standard() {
        let mut event = failed_usage_event(DefaultServiceTier::Fast);
        event.success = true;
        event.applied_service_tier = Some(DefaultServiceTier::Standard);
        event.requested_model = Some("gpt-5.6-sol".to_string());
        event.resolved_model = Some("gpt-5.6-sol".to_string());
        let api = ApiEquivalentSummary {
            micro_usd: 100_000,
            priced_tokens: 10_000,
            unpriced_tokens: 0,
        };

        let usage = QuotaEconomicsUsage::from_event(&event, api).unwrap();

        // The requested mode is what the account was billed for, so it decides
        // which tier the sample calibrates. The upstream tier is delivery
        // telemetry and must not move the sample to the other tier.
        assert_eq!(usage.service_tier, DefaultServiceTier::Fast);
        assert_eq!(usage.api_equivalent_micro_usd, Some(100_000));
    }

    #[test]
    fn stale_active_probe_is_not_treated_as_attributed_usage() {
        let mut state = QuotaEconomicsState::default();
        configure_account(&mut state, "plus", "test-revision");
        state.observe_quota(&snapshot(10_000, 300_000, 1_000));
        state.observe_usage_with_source(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
            2_000,
            QuotaObservationSource::Active,
        );
        state.observe_quota_with_source(
            &snapshot(9_700, 300_000, 62_001),
            QuotaObservationSource::Active,
        );

        assert!(state.secondary.samples.is_empty());
        assert_eq!(
            state
                .secondary
                .active_cycle
                .as_ref()
                .unwrap()
                .unattributed_basis_points,
            300
        );
    }

    #[test]
    fn passive_quota_observations_persist_source_precision_and_delta() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 300_000, 1_000));
        state.observe_usage_with_source(
            usage(1_000_000, 1, 10_000, DefaultServiceTier::Standard),
            None,
            2_000,
            QuotaObservationSource::Passive,
        );
        state.observe_quota_with_source(
            &snapshot(9_700, 300_000, 120_000),
            QuotaObservationSource::Passive,
        );

        assert_eq!(state.secondary.samples.len(), 1);
        let observation = state.secondary.observations.last().unwrap();
        assert_eq!(observation.source, QuotaObservationSource::Passive);
        assert_eq!(observation.delta_basis_points, -300);
        assert_eq!(observation.resolution_basis_points, 100);
        let restored: QuotaEconomicsState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(
            restored.secondary.observations,
            state.secondary.observations
        );
    }

    #[test]
    fn token_mix_change_does_not_create_a_quota_policy_epoch() {
        let mut state = QuotaEconomicsState::default();
        state.observe_quota(&snapshot(10_000, 30_000, 1_000));
        for (index, tokens) in [10_000, 10_000, 10_000, 100_000, 200_000]
            .into_iter()
            .enumerate()
        {
            state.observe_usage(usage(1_000, 1, tokens, DefaultServiceTier::Standard), None);
            let step = u16::try_from(index + 1).unwrap();
            state.observe_quota(&snapshot(
                10_000 - step * 300,
                30_000,
                2_000 + u64::from(step),
            ));
        }
        assert_eq!(state.secondary.epoch, 0);
        assert_eq!(state.secondary.samples.len(), 5);
        assert!(state.secondary.drift_samples.is_empty());
    }

    fn configure_account(state: &mut QuotaEconomicsState, plan: &str, revision: &str) {
        state.set_account_context("chatgpt", Some(plan));
        state.set_value_revision(revision);
    }

    fn complete_cycle(api_micro_usd: u64, completed_at_ms: u64) -> QuotaCycleRecord {
        QuotaCycleRecord {
            status: QuotaCycleStatus::Complete,
            provider: "chatgpt".to_string(),
            plan: Some("plus".to_string()),
            window_kind: QuotaWindowKind::Secondary,
            window_minutes: Some(10_080),
            fingerprint: format!("cycle-{completed_at_ms}"),
            started_at_ms: completed_at_ms.saturating_sub(500),
            completed_at_ms,
            reset_at_ms: Some(completed_at_ms),
            pricing_revision: Some("test-revision".to_string()),
            epoch: 1,
            service_tier: Some(DefaultServiceTier::Standard),
            standard_observations: 1,
            fast_observations: 0,
            active_observations: 1,
            passive_observations: 0,
            consumed_basis_points: 10_000,
            unattributed_basis_points: 0,
            api_equivalent_micro_usd: Some(api_micro_usd),
            requests: 1,
            input_tokens: 1,
            cached_input_tokens: 0,
            reasoning_tokens: 0,
            output_tokens: 1,
            total_tokens: 2,
        }
    }

    fn usage(
        micro_usd: u64,
        requests: u64,
        total_tokens: u64,
        service_tier: DefaultServiceTier,
    ) -> QuotaEconomicsUsage {
        QuotaEconomicsUsage {
            api_equivalent_micro_usd: Some(micro_usd),
            requests,
            input_tokens: total_tokens / 2,
            output_tokens: total_tokens / 2,
            total_tokens,
            service_tier,
            ..Default::default()
        }
    }

    fn failed_usage_event(service_tier: DefaultServiceTier) -> crate::UsageEvent {
        crate::UsageEvent {
            request_id: "request".to_string(),
            attempt: 1,
            local_key_id: "key".to_string(),
            source_id: "source".to_string(),
            candidate_id: Some("account".to_string()),
            account_id: Some("account".to_string()),
            routing: None,
            requested_model: Some("gpt-5.4".to_string()),
            resolved_model: Some("gpt-5.4".to_string()),
            wire_api: crate::WireApi::Responses,
            service_tier,
            applied_service_tier: Some(service_tier),
            success: false,
            http_status: 500,
            error_category: Some("upstream_error".to_string()),
            tool_use: crate::ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: None,
            latency_ms: 100,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(5_000),
            cached_input_tokens: Some(0),
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: Some(5_000),
            total_tokens: Some(10_000),
            quota_snapshot: None,
        }
    }

    fn snapshot(available: u16, reset_at_ms: u64, observed_at_ms: u64) -> QuotaSnapshot {
        snapshot_with_minutes(available, reset_at_ms, observed_at_ms, 10_080)
    }

    fn snapshot_with_minutes(
        available: u16,
        reset_at_ms: u64,
        observed_at_ms: u64,
        window_minutes: u32,
    ) -> QuotaSnapshot {
        QuotaSnapshot {
            secondary: Some(QuotaWindow {
                kind: QuotaWindowKind::Secondary,
                available_basis_points: Some(available),
                explicitly_full: None,
                reset_at_ms: Some(reset_at_ms),
                window_minutes: Some(window_minutes),
                observed_at_ms,
                full_transition_fingerprint: None,
            }),
            updated_at_ms: Some(observed_at_ms),
            ..Default::default()
        }
    }

    fn two_window_snapshot(
        primary_available: u16,
        secondary_available: u16,
        observed_at_ms: u64,
    ) -> QuotaSnapshot {
        QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                available_basis_points: Some(primary_available),
                explicitly_full: None,
                reset_at_ms: Some(30_000),
                window_minutes: Some(300),
                observed_at_ms,
                full_transition_fingerprint: None,
            }),
            secondary: Some(QuotaWindow {
                kind: QuotaWindowKind::Secondary,
                available_basis_points: Some(secondary_available),
                explicitly_full: None,
                reset_at_ms: Some(60_000),
                window_minutes: Some(10_080),
                observed_at_ms,
                full_transition_fingerprint: None,
            }),
            updated_at_ms: Some(observed_at_ms),
            ..Default::default()
        }
    }
}

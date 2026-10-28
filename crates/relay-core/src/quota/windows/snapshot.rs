use super::{QuotaWindow, QuotaWindowKind, SupplementalQuotaWindow};
use crate::error::safe_error_code;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaErrorState {
    pub code: String,
    pub occurred_at_ms: u64,
}

impl QuotaErrorState {
    pub fn new(code: &str, occurred_at_ms: u64) -> Self {
        Self {
            code: safe_error_code(code),
            occurred_at_ms,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub primary: Option<QuotaWindow>,
    pub secondary: Option<QuotaWindow>,
    #[serde(default)]
    pub supplemental: Vec<SupplementalQuotaWindow>,
    #[serde(default)]
    pub limit_reached: bool,
    pub reset_credits_available: Option<u32>,
    /// Provider-reported credits, expressed in millionths of one credit.
    /// A fresh positive balance keeps an otherwise exhausted account eligible.
    pub available_credits_micro_units: Option<u64>,
    /// A fresh provider ledger explicitly confirmed that credits can still be
    /// spent. This is separate from reset credits and customer billing.
    #[serde(default)]
    pub provider_credits_available: bool,
    #[serde(default)]
    pub provider_credits_unlimited: bool,
    /// Only a complete provider ledger observation advances this clock.
    /// Window headers and reported exhaustion must not refresh old credits.
    #[serde(default)]
    pub provider_credits_observed_at_ms: Option<u64>,
    #[serde(default)]
    pub direct_balance_micro_usd: Option<u64>,
    pub updated_at_ms: Option<u64>,
    pub error: Option<QuotaErrorState>,
}

impl QuotaSnapshot {
    pub fn has_usable_provider_credits(&self) -> bool {
        self.provider_credits_available
    }

    pub fn has_fresh_provider_credits(&self, now_ms: u64, stale_after_ms: u64) -> bool {
        self.has_usable_provider_credits()
            && crate::quota::observation_is_fresh(
                self.provider_credits_observed_at_ms,
                now_ms,
                stale_after_ms,
            )
    }

    /// The oldest known remainder bounds the freshness of the combined quota.
    /// A reset-only header is not a new observation of that remainder.
    pub fn quota_observed_at_ms(&self) -> Option<u64> {
        self.primary
            .iter()
            .chain(self.secondary.iter())
            .filter(|window| window.available_basis_points.is_some())
            .map(|window| window.observed_at_ms)
            .min()
            .or(self.updated_at_ms)
    }

    /// Reconcile the ledger independently of newer passive window headers.
    /// Unknown/older ledgers cannot overwrite a later complete observation.
    pub fn merge_provider_credits(&mut self, incoming: &Self) -> bool {
        let Some(incoming_at_ms) = incoming.provider_credits_observed_at_ms else {
            return false;
        };
        if self
            .provider_credits_observed_at_ms
            .is_some_and(|current_at_ms| current_at_ms >= incoming_at_ms)
        {
            return false;
        }
        self.copy_provider_credits(incoming);
        true
    }

    fn copy_provider_credits(&mut self, incoming: &Self) {
        self.available_credits_micro_units = incoming.available_credits_micro_units;
        self.provider_credits_available = incoming.provider_credits_available;
        self.provider_credits_unlimited = incoming.provider_credits_unlimited;
        self.provider_credits_observed_at_ms = incoming.provider_credits_observed_at_ms;
    }

    /// Queued usage may carry newer windows with an older ledger. Reconcile
    /// each resource by its own clock instead of rolling the ledger back.
    pub fn merge_observation(&mut self, incoming: &Self) {
        self.merge_observation_with_preference(
            incoming,
            incoming.updated_at_ms.unwrap_or_default() >= self.updated_at_ms.unwrap_or_default(),
        );
    }

    /// Snapshot metadata and missing windows follow the caller's ordering policy.
    /// Present windows and complete ledgers retain their independent clocks;
    /// conflicting observations at the same resource timestamp keep current data.
    pub(crate) fn merge_observation_with_preference(
        &mut self,
        incoming: &Self,
        incoming_wins: bool,
    ) -> bool {
        let mut merged = if incoming_wins {
            incoming.clone()
        } else {
            self.clone()
        };
        for (current, incoming, target) in [
            (&self.primary, &incoming.primary, &mut merged.primary),
            (&self.secondary, &incoming.secondary, &mut merged.secondary),
        ] {
            if let (Some(current), Some(incoming)) = (current, incoming) {
                *target = Some(merge_window_observation(current, incoming, incoming_wins));
            }
        }
        for target in &mut merged.supplemental {
            let current = self
                .supplemental
                .iter()
                .find(|window| window.id == target.id);
            let incoming = incoming
                .supplemental
                .iter()
                .find(|window| window.id == target.id);
            if let (Some(current), Some(incoming)) = (current, incoming) {
                target.window =
                    merge_window_observation(&current.window, &incoming.window, incoming_wins);
            }
        }
        // Start with current ledger evidence, including equal-clock conflicts.
        // Legacy undated values still follow the preferred snapshot.
        if self.provider_credits_observed_at_ms.is_some() {
            merged.copy_provider_credits(self);
        }
        merged.merge_provider_credits(incoming);
        let changed = merged != *self;
        *self = merged;
        changed
    }

    pub fn window(&self, kind: QuotaWindowKind) -> Option<&QuotaWindow> {
        match kind {
            QuotaWindowKind::Primary => self.primary.as_ref(),
            QuotaWindowKind::Secondary => self.secondary.as_ref(),
        }
    }

    pub fn limiting_reset_at_ms(&self) -> Option<u64> {
        self.primary
            .iter()
            .chain(self.secondary.iter())
            .filter_map(|window| Some((window.available_basis_points?, window.reset_at_ms?)))
            .min_by_key(|(available, reset_at_ms)| (*available, *reset_at_ms))
            .map(|(_, reset_at_ms)| reset_at_ms)
    }

    /// Records that the provider reported the primary window exhausted without
    /// sending a snapshot. An already-open primary window is zeroed. Missing
    /// windows are not invented, and the secondary window is left alone so a
    /// false weekly zero cannot spend a reset credit.
    pub fn note_reported_window_exhaustion(&mut self, observed_at_ms: u64) -> bool {
        let Some(primary) = self.primary.as_mut() else {
            return false;
        };
        if observed_at_ms < primary.observed_at_ms
            || primary
                .available_basis_points
                .is_none_or(|available| available == 0)
        {
            return false;
        }
        primary.available_basis_points = Some(0);
        primary.explicitly_full = Some(false);
        primary.observed_at_ms = primary.observed_at_ms.max(observed_at_ms);
        self.updated_at_ms = Some(self.updated_at_ms.unwrap_or_default().max(observed_at_ms));
        true
    }
}

fn merge_window_observation(
    current: &QuotaWindow,
    incoming: &QuotaWindow,
    incoming_wins: bool,
) -> QuotaWindow {
    if incoming.observed_at_ms > current.observed_at_ms {
        return incoming.clone();
    }
    let mut merged = current.clone();
    // Timer-only headers intentionally retain the remainder's observation clock.
    // Let newer metadata refine that timer without replacing conflicting quota.
    if incoming_wins
        && incoming.observed_at_ms == current.observed_at_ms
        && incoming.available_basis_points == current.available_basis_points
    {
        merged.reset_at_ms = incoming.reset_at_ms;
        merged.window_minutes = incoming.window_minutes;
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::{QuotaWindow, QuotaWindowKind};

    fn open_window(kind: QuotaWindowKind, available: u16) -> QuotaWindow {
        QuotaWindow {
            kind,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(available),
            explicitly_full: None,
            reset_at_ms: Some(50_000),
            window_minutes: Some(300),
            observed_at_ms: 1_000,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }
    }

    #[test]
    fn ledger_reconciliation_preserves_windows_and_rejects_old_or_unknown_credits() {
        let mut quota = QuotaSnapshot {
            primary: Some(open_window(QuotaWindowKind::Primary, 900)),
            available_credits_micro_units: Some(100),
            provider_credits_available: true,
            provider_credits_observed_at_ms: Some(1_000),
            updated_at_ms: Some(3_000),
            ..Default::default()
        };
        let incoming = QuotaSnapshot {
            available_credits_micro_units: Some(0),
            provider_credits_observed_at_ms: Some(2_000),
            ..Default::default()
        };
        assert!(quota.merge_provider_credits(&incoming));
        assert_eq!(quota.available_credits_micro_units, Some(0));
        assert!(!quota.provider_credits_available);
        assert_eq!(quota.updated_at_ms, Some(3_000));
        assert_eq!(
            quota.primary.as_ref().unwrap().available_basis_points,
            Some(900)
        );
        assert!(!quota.merge_provider_credits(&incoming));
        assert!(!quota.merge_provider_credits(&QuotaSnapshot {
            provider_credits_available: true,
            available_credits_micro_units: Some(1_000_000),
            provider_credits_observed_at_ms: Some(1_000),
            ..Default::default()
        }));
        assert!(!quota.merge_provider_credits(&QuotaSnapshot::default()));
        assert_eq!(quota.available_credits_micro_units, Some(0));
        quota.merge_observation(&QuotaSnapshot {
            updated_at_ms: Some(4_000),
            provider_credits_available: true,
            available_credits_micro_units: Some(1_000_000),
            provider_credits_observed_at_ms: Some(1_000),
            ..Default::default()
        });
        assert_eq!(quota.updated_at_ms, Some(4_000));
        assert_eq!(quota.provider_credits_observed_at_ms, Some(2_000));
        assert_eq!(quota.available_credits_micro_units, Some(0));
        quota.merge_observation(&QuotaSnapshot {
            updated_at_ms: Some(3_500),
            provider_credits_available: true,
            available_credits_micro_units: Some(700),
            provider_credits_observed_at_ms: Some(3_000),
            ..Default::default()
        });
        assert_eq!(quota.updated_at_ms, Some(4_000));
        assert_eq!(quota.provider_credits_observed_at_ms, Some(3_000));
        assert_eq!(quota.available_credits_micro_units, Some(700));
    }

    #[test]
    fn queued_windows_merge_by_their_own_clocks_not_the_snapshot_clock() {
        let mut quota = QuotaSnapshot {
            primary: Some(open_window(QuotaWindowKind::Primary, 0)),
            secondary: Some(open_window(QuotaWindowKind::Secondary, 7000)),
            updated_at_ms: Some(3000),
            ..Default::default()
        };
        quota.primary.as_mut().unwrap().observed_at_ms = 3000;
        let mut incoming = QuotaSnapshot {
            primary: Some(open_window(QuotaWindowKind::Primary, 9000)),
            secondary: Some(open_window(QuotaWindowKind::Secondary, 6000)),
            updated_at_ms: Some(4000),
            ..Default::default()
        };
        incoming.secondary.as_mut().unwrap().observed_at_ms = 4000;
        quota.merge_observation(&incoming);
        assert_eq!(
            quota.primary.as_ref().unwrap().available_basis_points,
            Some(0)
        );
        assert_eq!(quota.primary.as_ref().unwrap().observed_at_ms, 3000);
        assert_eq!(
            quota.secondary.as_ref().unwrap().available_basis_points,
            Some(6000)
        );

        incoming.primary.as_mut().unwrap().observed_at_ms = 3500;
        incoming.secondary.as_mut().unwrap().observed_at_ms = 1000;
        incoming.secondary.as_mut().unwrap().available_basis_points = Some(9000);
        incoming.updated_at_ms = Some(3500);
        quota.merge_observation(&incoming);
        assert_eq!(
            quota.primary.as_ref().unwrap().available_basis_points,
            Some(9000)
        );
        assert_eq!(
            quota.secondary.as_ref().unwrap().available_basis_points,
            Some(6000)
        );
        assert_eq!(quota.updated_at_ms, Some(4000));
    }

    #[test]
    fn a_new_snapshot_clock_does_not_overwrite_equal_clock_ledger_evidence() {
        let mut quota = QuotaSnapshot {
            available_credits_micro_units: Some(0),
            provider_credits_available: false,
            provider_credits_observed_at_ms: Some(3000),
            updated_at_ms: Some(3000),
            ..Default::default()
        };
        quota.merge_observation(&QuotaSnapshot {
            available_credits_micro_units: Some(1000),
            provider_credits_available: true,
            provider_credits_observed_at_ms: Some(3000),
            updated_at_ms: Some(4000),
            ..Default::default()
        });
        assert_eq!(quota.available_credits_micro_units, Some(0));
        assert!(!quota.provider_credits_available);
        assert_eq!(quota.updated_at_ms, Some(4000));
    }

    #[test]
    fn delayed_exhaustion_cannot_zero_a_newer_primary_window() {
        let mut quota = QuotaSnapshot {
            primary: Some(open_window(QuotaWindowKind::Primary, 9000)),
            updated_at_ms: Some(3000),
            ..Default::default()
        };
        quota.primary.as_mut().unwrap().observed_at_ms = 3000;
        let before = quota.clone();
        assert!(!quota.note_reported_window_exhaustion(2000));
        assert_eq!(quota, before);
        assert!(quota.note_reported_window_exhaustion(4000));
        assert_eq!(
            quota.primary.as_ref().unwrap().available_basis_points,
            Some(0)
        );
    }

    #[test]
    fn equal_clock_windows_preserve_current_evidence() {
        let mut quota = QuotaSnapshot {
            primary: Some(open_window(QuotaWindowKind::Primary, 0)),
            supplemental: vec![SupplementalQuotaWindow {
                id: "extra".into(),
                label: "Extra".into(),
                service_tier: None,
                window: open_window(QuotaWindowKind::Secondary, 500),
            }],
            updated_at_ms: Some(1_000),
            ..Default::default()
        };
        let mut incoming = quota.clone();
        incoming.primary.as_mut().unwrap().available_basis_points = Some(9_000);
        incoming.supplemental[0].window.available_basis_points = Some(9_000);
        incoming.updated_at_ms = Some(2_000);
        quota.merge_observation(&incoming);
        assert_eq!(
            quota.primary.as_ref().unwrap().available_basis_points,
            Some(0)
        );
        assert_eq!(
            quota.supplemental[0].window.available_basis_points,
            Some(500)
        );
        incoming.supplemental[0].window.observed_at_ms = 2_000;
        quota.merge_observation(&incoming);
        assert_eq!(
            quota.supplemental[0].window.available_basis_points,
            Some(9_000)
        );
    }

    #[test]
    fn removed_windows_are_not_restored_from_an_older_snapshot() {
        let mut quota = QuotaSnapshot {
            primary: Some(open_window(QuotaWindowKind::Primary, 900)),
            secondary: Some(open_window(QuotaWindowKind::Secondary, 500)),
            updated_at_ms: Some(1_000),
            ..Default::default()
        };
        let older = quota.clone();
        quota.merge_observation(&QuotaSnapshot {
            updated_at_ms: Some(2_000),
            ..Default::default()
        });
        assert!(quota.primary.is_none());
        assert!(quota.secondary.is_none());
        quota.merge_observation(&older);
        assert!(quota.primary.is_none());
        assert!(quota.secondary.is_none());
        assert_eq!(quota.updated_at_ms, Some(2_000));
    }

    #[test]
    fn newer_reset_metadata_keeps_the_carried_remainder_clock() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-codex-primary-used-percent", "20".parse().unwrap());
        headers.insert(
            "x-codex-primary-reset-after-seconds",
            "600".parse().unwrap(),
        );
        let mut quota = crate::providers::chatgpt::merge_codex_quota_headers(
            &QuotaSnapshot::default(),
            &headers,
            1_000,
        )
        .unwrap();
        headers.remove("x-codex-primary-used-percent");
        let timer =
            crate::providers::chatgpt::merge_codex_quota_headers(&quota, &headers, 2_000).unwrap();
        quota.merge_observation(&timer);
        let primary = quota.primary.as_ref().unwrap();
        assert_eq!(primary.available_basis_points, Some(8_000));
        assert_eq!(primary.observed_at_ms, 1_000);
        assert_eq!(primary.reset_at_ms, Some(602_000));
        assert_eq!(quota.updated_at_ms, Some(2_000));
    }

    #[test]
    fn legacy_snapshots_keep_their_balance_without_inventing_ledger_freshness() {
        let quota: QuotaSnapshot = serde_json::from_value(serde_json::json!({
            "availableCreditsMicroUnits": 100,
            "providerCreditsAvailable": true,
            "updatedAtMs": 1_000
        }))
        .unwrap();
        assert_eq!(quota.available_credits_micro_units, Some(100));
        assert!(!quota.has_fresh_provider_credits(1_001, 60_000));
    }

    #[test]
    fn reported_exhaustion_zeroes_only_an_open_primary_window() {
        let mut quota = QuotaSnapshot {
            primary: Some(open_window(QuotaWindowKind::Primary, 900)),
            secondary: Some(open_window(QuotaWindowKind::Secondary, 2_900)),
            updated_at_ms: Some(1_000),
            ..QuotaSnapshot::default()
        };
        assert!(quota.note_reported_window_exhaustion(2_000));
        let primary = quota.primary.as_ref().unwrap();
        assert_eq!(primary.available_basis_points, Some(0));
        assert_eq!(primary.explicitly_full, Some(false));
        assert_eq!(primary.observed_at_ms, 2_000);
        assert_eq!(primary.reset_at_ms, Some(50_000));
        assert_eq!(
            quota.secondary.as_ref().unwrap().available_basis_points,
            Some(2_900)
        );
        assert!(!quota.limit_reached);
        assert_eq!(quota.updated_at_ms, Some(2_000));
        assert!(!quota.note_reported_window_exhaustion(3_000));

        let mut empty = QuotaSnapshot::default();
        assert!(!empty.note_reported_window_exhaustion(3_000));
        assert!(empty.primary.is_none());
    }
}

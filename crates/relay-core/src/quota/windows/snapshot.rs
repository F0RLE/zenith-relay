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
    #[serde(default)]
    pub direct_balance_micro_usd: Option<u64>,
    pub updated_at_ms: Option<u64>,
    pub error: Option<QuotaErrorState>,
}

impl QuotaSnapshot {
    pub fn has_usable_provider_credits(&self) -> bool {
        self.provider_credits_available
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
        if primary
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

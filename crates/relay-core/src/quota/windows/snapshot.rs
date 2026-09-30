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
}

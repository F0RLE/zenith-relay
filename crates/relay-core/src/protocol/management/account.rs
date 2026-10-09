use super::{AccountRoutingBlockReason, OperationalStatus, ProxyMode, QuotaRefreshStatus};
use crate::{
    accounts::AccountAuthState,
    quota::{QuotaSnapshot, Subscription},
    scheduler::refresh::RefreshFreshness,
    ApiEquivalentSummary,
};
use serde::{Deserialize, Serialize};
use std::fmt;

mod quota;
mod source;

pub use quota::{api_equivalent_projection_window, QuotaWindowUsage};
pub use source::{SourceRefreshState, SourceSummary, SourceSummaryRecord};

/// Refresh evidence, not an inference eligibility or health decision. The
/// coordinator's as-of clock is monotonic and must not be sent as wall time.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshStatus {
    #[default]
    Unknown,
    Fresh,
    Stale,
    Unsupported,
}

impl RefreshStatus {
    /// Saved observations survive restart, but a newly created coordinator
    /// cannot claim they have been revalidated in this process.
    pub fn from_evidence(freshness: RefreshFreshness, saved_value: bool) -> Self {
        match freshness {
            RefreshFreshness::Unknown if saved_value => Self::Stale,
            RefreshFreshness::Unknown => Self::Unknown,
            RefreshFreshness::Fresh { .. } => Self::Fresh,
            RefreshFreshness::Stale { .. } => Self::Stale,
            RefreshFreshness::Unsupported => Self::Unsupported,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRefreshState {
    pub models: RefreshStatus,
    pub quota: RefreshStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccountLocation {
    pub server_id: String,
    pub remote_account_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    #[serde(default)]
    pub oauth_client_kind: crate::providers::chatgpt::OAuthClientKind,
    pub id: String,
    pub label: String,
    pub identity_hint: String,
    /// Hashed provider ledger identity, independent of the issuing OAuth client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_balance_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_family: Option<String>,
    /// Basis Points is available only for an Excel/Basis Points OAuth connection.
    #[serde(default)]
    pub basis_points_available: bool,
    #[serde(default)]
    pub basis_points_enabled: bool,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub operational_status: OperationalStatus,
    pub auth_state: AccountAuthState,
    pub health: String,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_window_usage: Option<QuotaWindowUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_cost_micro_usd: Option<u64>,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    #[serde(default)]
    pub quota_refresh_status: QuotaRefreshStatus,
    /// Quota and model evidence remain independent of account auth/health.
    #[serde(default)]
    pub refresh_state: AccountRefreshState,
    pub secret_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_location: Option<RemoteAccountLocation>,
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    #[serde(default)]
    pub proxy_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_block_reason: Option<AccountRoutingBlockReason>,
    pub last_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_auth_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_client_login_redirect_at_ms: Option<u64>,
}

pub fn model_has_native_account_route(accounts: &[AccountSummary], model: &str) -> bool {
    accounts.iter().any(|account| {
        account.in_pool
            && account.oauth_client_kind == crate::providers::chatgpt::OAuthClientKind::Codex
            && account
                .models
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(model))
    })
}

#[cfg(test)]
mod refresh_projection_tests {
    use super::*;

    #[test]
    fn saved_values_are_stale_after_restart_but_not_unknown_or_unsupported_resources() {
        assert_eq!(
            RefreshStatus::from_evidence(RefreshFreshness::Unknown, true),
            RefreshStatus::Stale
        );
        assert_eq!(
            RefreshStatus::from_evidence(RefreshFreshness::Unknown, false),
            RefreshStatus::Unknown
        );
        assert_eq!(
            RefreshStatus::from_evidence(RefreshFreshness::Unsupported, true),
            RefreshStatus::Unsupported
        );
        assert_eq!(
            RefreshStatus::from_evidence(RefreshFreshness::Fresh { as_of_ms: 1 }, false),
            RefreshStatus::Fresh
        );
        assert_eq!(
            RefreshStatus::from_evidence(RefreshFreshness::Stale { as_of_ms: 1 }, false),
            RefreshStatus::Stale
        );
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealedAccountIdentity {
    pub account_id: String,
    pub identity: String,
}

impl fmt::Debug for RevealedAccountIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RevealedAccountIdentity")
            .field("account_id", &self.account_id)
            .field("identity", &"[redacted]")
            .finish()
    }
}

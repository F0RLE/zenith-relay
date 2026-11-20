//! Opt-in evidence for a reset attempt and its separate quota verification.
use super::{AccountQuotaOutcome, AccountQuotaRefreshResponse, StatusCode};
use crate::diagnostics::{DebugSession, hash_identifier, record_operation};
use uuid::Uuid;
use zenith_relay_core::quota::QuotaSnapshot;

pub(crate) enum ResetOrigin<'a> {
    Manual(&'a QuotaSnapshot),
    WeeklyReserve(&'a QuotaSnapshot),
    WeeklyThreshold(&'a QuotaSnapshot),
}

pub(crate) struct ResetAttempt<'a> {
    pub(super) request_id: String,
    debug: Option<DebugSession>,
    origin: ResetOrigin<'a>,
}

impl<'a> ResetAttempt<'a> {
    pub(crate) fn new(origin: ResetOrigin<'a>) -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            debug: DebugSession::capture(),
            origin,
        }
    }

    pub(super) fn record_dispatch(&self, account_id: &str) {
        let quota = match self.origin {
            ResetOrigin::Manual(quota)
            | ResetOrigin::WeeklyReserve(quota)
            | ResetOrigin::WeeklyThreshold(quota) => quota,
        };
        self.record(account_id, "post_started", Some(quota), None);
    }

    pub(super) fn record_post_result(&self, account_id: &str, status: Option<StatusCode>) {
        let stage = match status {
            Some(status) if status.is_success() => "provider_accepted",
            Some(_) => "provider_non_success",
            None => "post_unconfirmed",
        };
        self.record(account_id, stage, None, status);
    }

    pub(crate) fn record_verification(
        &self,
        account_id: &str,
        response: Option<&AccountQuotaRefreshResponse>,
    ) {
        let quota = response
            .filter(|response| matches!(response.quota, AccountQuotaOutcome::Updated { .. }))
            .map(|response| &response.account.account.quota);
        let stage = if quota.is_some() {
            "quota_observed_after"
        } else {
            "quota_unverified"
        };
        self.record(account_id, stage, quota, None);
    }

    fn record(
        &self,
        account_id: &str,
        stage: &str,
        quota: Option<&QuotaSnapshot>,
        status: Option<StatusCode>,
    ) {
        if !self.debug.is_some_and(DebugSession::is_current) {
            return;
        }
        let mode = match self.origin {
            ResetOrigin::Manual(_) => "manual",
            ResetOrigin::WeeklyReserve(_) => "weekly_reserve",
            ResetOrigin::WeeklyThreshold(_) => "weekly_threshold",
        };
        let mut details = vec![
            ("account", hash_identifier(account_id)),
            ("attempt", hash_identifier(&self.request_id)),
            ("mode", mode.into()),
        ];
        if let Some(quota) = quota {
            let values = [
                (
                    "primary_remaining_bp",
                    quota
                        .primary
                        .as_ref()
                        .and_then(|w| w.available_basis_points)
                        .map(u64::from),
                ),
                (
                    "secondary_remaining_bp",
                    quota
                        .secondary
                        .as_ref()
                        .and_then(|w| w.available_basis_points)
                        .map(u64::from),
                ),
                (
                    "secondary_window_minutes",
                    quota
                        .secondary
                        .as_ref()
                        .and_then(|w| w.window_minutes)
                        .map(u64::from),
                ),
                (
                    "secondary_reset_at_ms",
                    quota.secondary.as_ref().and_then(|w| w.reset_at_ms),
                ),
                (
                    "observed_reset_credits",
                    quota.reset_credits_available.map(u64::from),
                ),
            ];
            for (key, value) in values {
                if let Some(value) = value {
                    details.push((key, value.to_string()));
                }
            }
        }
        if let Some(status) = status {
            details.push(("http_status", status.as_u16().to_string()));
        }
        record_operation("reset-credit", stage, &details);
    }
}

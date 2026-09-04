use crate::{
    account_candidate_health,
    accounts::{AccountAuthState, AccountHealthState},
    quota::{QuotaSnapshot, Subscription, SubscriptionStatus},
    CandidateHealth, CandidateQuota,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Direct,
    Common,
    Account,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRoutingBlockReason {
    Disabled,
    NotInPool,
    Draining,
    SecretUnavailable,
    ProxyUnavailable,
    ReauthRequired,
    AuthError,
    Checkpoint,
    Captcha,
    SubscriptionForbidden,
    SubscriptionExpired,
    AccountUnhealthy,
    QuotaExhausted,
}

pub struct AccountOperationalInput<'a> {
    pub enabled: bool,
    pub in_pool: bool,
    pub draining: bool,
    pub secret_available: bool,
    pub proxy_available: bool,
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub subscription: &'a Subscription,
    pub quota: &'a QuotaSnapshot,
    pub last_error_code: Option<&'a str>,
    pub now_ms: u64,
    pub quota_stale_after_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountOperationalState {
    pub status: OperationalStatus,
    pub health: CandidateHealth,
    pub quota: CandidateQuota,
    pub routing_eligible: bool,
    pub routing_block_reason: Option<AccountRoutingBlockReason>,
}

/// Whether an account should remain instantiated as a runtime candidate.
///
/// An exhausted quota is a temporary scheduler condition rather than a broken
/// configuration. Keeping that candidate lets a quota refresh restore it in
/// place without rebuilding the gateway or dropping active work.
pub fn account_candidate_enabled(
    account_enabled: bool,
    routing_block_reason: Option<AccountRoutingBlockReason>,
) -> bool {
    account_enabled
        && matches!(
            routing_block_reason,
            None | Some(AccountRoutingBlockReason::QuotaExhausted)
        )
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaRefreshStatus {
    #[default]
    Pending,
    Refreshing,
    Updated,
    Failed,
    RequiresReauth,
}

pub fn quota_refresh_status(
    auth_state: AccountAuthState,
    quota: &QuotaSnapshot,
    refreshing: bool,
) -> QuotaRefreshStatus {
    if auth_state.requires_fresh_login() {
        QuotaRefreshStatus::RequiresReauth
    } else if refreshing {
        QuotaRefreshStatus::Refreshing
    } else if quota.error.is_some() {
        QuotaRefreshStatus::Failed
    } else if quota.updated_at_ms.is_some() {
        QuotaRefreshStatus::Updated
    } else {
        QuotaRefreshStatus::Pending
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationalStatus {
    Rotation,
    QuotaWait,
    Unavailable,
    Disabled,
}

pub fn operational_status(
    enabled: bool,
    quota_wait: bool,
    configured_available: bool,
    runtime_available: Option<bool>,
) -> OperationalStatus {
    if !enabled {
        return OperationalStatus::Disabled;
    }
    if !configured_available {
        return OperationalStatus::Unavailable;
    }
    if quota_wait {
        return OperationalStatus::QuotaWait;
    }
    if runtime_available.unwrap_or(configured_available) {
        OperationalStatus::Rotation
    } else {
        OperationalStatus::Unavailable
    }
}

pub fn account_operational_state(input: AccountOperationalInput<'_>) -> AccountOperationalState {
    let health = account_candidate_health(
        input.auth_state,
        input.health,
        input.subscription.status,
        input.last_error_code,
    );
    let quota =
        CandidateQuota::from_snapshot(input.quota, input.now_ms, input.quota_stale_after_ms);
    let configured_available =
        !input.draining && input.secret_available && input.proxy_available && health.is_eligible();
    let status = operational_status(
        input.enabled,
        quota == CandidateQuota::Exhausted,
        configured_available,
        None,
    );
    let routing_block_reason = account_routing_block_reason(&input, health, quota);
    AccountOperationalState {
        status,
        health,
        quota,
        routing_eligible: routing_block_reason.is_none(),
        routing_block_reason,
    }
}

fn account_routing_block_reason(
    input: &AccountOperationalInput<'_>,
    health: CandidateHealth,
    quota: CandidateQuota,
) -> Option<AccountRoutingBlockReason> {
    if !input.enabled {
        return Some(AccountRoutingBlockReason::Disabled);
    }
    if !input.in_pool {
        return Some(AccountRoutingBlockReason::NotInPool);
    }
    if input.draining {
        return Some(AccountRoutingBlockReason::Draining);
    }
    if !input.secret_available {
        return Some(AccountRoutingBlockReason::SecretUnavailable);
    }
    if !input.proxy_available {
        return Some(AccountRoutingBlockReason::ProxyUnavailable);
    }
    if input.auth_state.requires_fresh_login() {
        return Some(AccountRoutingBlockReason::ReauthRequired);
    }
    if input.auth_state == AccountAuthState::Error {
        return Some(AccountRoutingBlockReason::AuthError);
    }
    match input.last_error_code {
        Some("checkpoint" | "upstream_account_verification_required") => {
            return Some(AccountRoutingBlockReason::Checkpoint)
        }
        Some("captcha") => return Some(AccountRoutingBlockReason::Captcha),
        _ => {}
    }
    match input.subscription.status {
        SubscriptionStatus::Forbidden => {
            return Some(AccountRoutingBlockReason::SubscriptionForbidden)
        }
        // ChatGPT Team/Business may continue serving Codex after the
        // entitlement date exposed by the auxiliary subscription endpoint has
        // gone stale. Only an explicit forbidden signal blocks routing;
        // successful /wham/usage and Responses results remain authoritative.
        SubscriptionStatus::Expired => {}
        _ => {}
    }
    if !health.is_eligible() {
        return Some(AccountRoutingBlockReason::AccountUnhealthy);
    }
    (quota == CandidateQuota::Exhausted).then_some(AccountRoutingBlockReason::QuotaExhausted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::ReauthReason;

    #[test]
    fn legacy_reused_refresh_token_does_not_request_login_or_block_routing() {
        let auth_state = AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken);
        let quota = QuotaSnapshot::default();
        let subscription = Subscription::default();

        assert_eq!(
            quota_refresh_status(auth_state, &quota, false),
            QuotaRefreshStatus::Pending
        );

        let state = account_operational_state(AccountOperationalInput {
            enabled: true,
            in_pool: true,
            draining: false,
            secret_available: true,
            proxy_available: true,
            auth_state,
            health: AccountHealthState::Healthy,
            subscription: &subscription,
            quota: &quota,
            last_error_code: None,
            now_ms: 0,
            quota_stale_after_ms: 60_000,
        });
        assert!(state.routing_eligible);
        assert_eq!(state.routing_block_reason, None);
    }
}

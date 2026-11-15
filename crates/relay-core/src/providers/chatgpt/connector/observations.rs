//! Provider observations are reduced independently of host persistence and UI events.
use crate::accounts::{AccountQuotaUpdate, AccountRecord, reduce_account_quota};
use crate::providers::chatgpt::QuotaRefreshOutcome;
use crate::quota::{
    QuotaRefreshFailure, QuotaRefreshResult, QuotaSnapshot, QuotaTransition, Subscription,
};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedQuota {
    pub transitions: Vec<QuotaTransition>,
    pub exhaustion_transitions: Vec<QuotaTransition>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum QuotaReadOutcome {
    Skipped,
    Updated {
        transitions: Vec<QuotaTransition>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exhaustion_transitions: Vec<QuotaTransition>,
    },
    Failed {
        code: String,
        retryable: bool,
    },
}

pub enum SubscriptionObservation {
    Timestamped(Subscription),
    Partial(Subscription),
}

impl From<Subscription> for SubscriptionObservation {
    fn from(subscription: Subscription) -> Self {
        Self::Timestamped(subscription)
    }
}

fn apply_update(account: &mut AccountRecord, update: AccountQuotaUpdate) {
    account.quota = update.quota;
    account.subscription = update.subscription;
    account.health = update.health;
    account.last_error_code = update.last_error_code;
}

pub fn apply_quota_success(
    account: &mut AccountRecord,
    result: QuotaRefreshResult,
) -> Result<AppliedQuota, &'static str> {
    let now_ms = result.quota.observed_at_ms;
    let update = reduce_account_quota(
        &account.quota,
        &account.subscription,
        account.health,
        account.last_error_code.as_deref(),
        Ok(result),
        now_ms,
    )
    .map_err(|_| "quota response could not be normalized")?;
    let transitions = match &update.outcome {
        crate::accounts::AccountQuotaOutcome::Updated { transitions } => transitions.clone(),
        crate::accounts::AccountQuotaOutcome::Failed { .. } => {
            return Err("quota result kind is invalid");
        }
    };
    let exhaustion_transitions = update.exhaustion_transitions.clone();
    apply_update(account, update);
    Ok(AppliedQuota {
        transitions,
        exhaustion_transitions,
    })
}

pub fn apply_quota_failure(
    account: &mut AccountRecord,
    failure: &QuotaRefreshFailure,
    now_ms: u64,
) {
    let update = reduce_account_quota(
        &account.quota,
        &account.subscription,
        account.health,
        account.last_error_code.as_deref(),
        Err(failure.clone()),
        now_ms,
    )
    .expect("quota failure reduction does not normalize provider data");
    apply_update(account, update);
}

pub fn apply_quota_outcome(
    account: &mut AccountRecord,
    outcome: QuotaRefreshOutcome,
    now_ms: u64,
) -> (QuotaReadOutcome, Vec<QuotaTransition>) {
    match outcome {
        QuotaRefreshOutcome::Updated(quota_data) => match apply_quota_success(account, *quota_data)
        {
            Ok(applied) => (
                QuotaReadOutcome::Updated {
                    transitions: applied.transitions,
                    exhaustion_transitions: applied.exhaustion_transitions.clone(),
                },
                applied.exhaustion_transitions,
            ),
            Err(_) => {
                let failure =
                    QuotaRefreshFailure::new(crate::error_codes::QUOTA_INVALID_RESPONSE, false);
                apply_quota_failure(account, &failure, now_ms);
                (
                    QuotaReadOutcome::Failed {
                        code: failure.code,
                        retryable: failure.retryable,
                    },
                    Vec::new(),
                )
            }
        },
        QuotaRefreshOutcome::Failed {
            failure,
            subscription,
        } => {
            account.subscription = subscription;
            apply_quota_failure(account, &failure, now_ms);
            (
                QuotaReadOutcome::Failed {
                    code: failure.code,
                    retryable: failure.retryable,
                },
                Vec::new(),
            )
        }
    }
}

/// Reset timer drift alone is not newer usage. Balances and limit evidence are.
pub fn quota_is_newer(current: &QuotaSnapshot, initial: &QuotaSnapshot) -> bool {
    let remaining = |window: &Option<crate::quota::QuotaWindow>| {
        window.as_ref().and_then(|w| w.available_basis_points)
    };
    remaining(&current.primary) != remaining(&initial.primary)
        || remaining(&current.secondary) != remaining(&initial.secondary)
        || current.limit_reached != initial.limit_reached
        || current.reset_credits_available != initial.reset_credits_available
        || current.available_credits_micro_units != initial.available_credits_micro_units
        || current.provider_credits_available != initial.provider_credits_available
        || current.provider_credits_unlimited != initial.provider_credits_unlimited
        || current.provider_credits_observed_at_ms != initial.provider_credits_observed_at_ms
        || current.direct_balance_micro_usd != initial.direct_balance_micro_usd
        || !current
            .supplemental
            .iter()
            .map(|w| (&w.id, w.window.available_basis_points))
            .eq(initial
                .supplemental
                .iter()
                .map(|w| (&w.id, w.window.available_basis_points)))
}

/// The caller verifies its durable account revision before applying this reducer.
/// Preserve concurrent auth/health changes after model reduction using the same snapshot.
pub fn apply_quota_read(
    account: &mut AccountRecord,
    initial: &AccountRecord,
    quota: QuotaRefreshOutcome,
    subscription: impl Into<SubscriptionObservation>,
    started_at_ms: u64,
) -> (QuotaReadOutcome, Vec<QuotaTransition>) {
    let before = account.clone();
    let (subscription, partial) = match subscription.into() {
        SubscriptionObservation::Timestamped(subscription) => (subscription, false),
        SubscriptionObservation::Partial(subscription) => (subscription, true),
    };
    let observed = match &quota {
        QuotaRefreshOutcome::Updated(result) => result
            .quota
            .subscription
            .clone()
            .map(Subscription::normalize),
        QuotaRefreshOutcome::Failed { subscription, .. } => Some(subscription.clone()),
    };
    let subscription = observed
        .filter(|observed| {
            !partial
                && (observed.updated_at_ms > subscription.updated_at_ms
                    || (observed.updated_at_ms == subscription.updated_at_ms
                        && (observed.active_until_ms.is_some()
                            || subscription.active_until_ms.is_none()
                            || crate::quota::subscription_plan_changed(
                                subscription.plan_type.as_deref(),
                                observed.plan_type.as_deref(),
                            ))))
        })
        .unwrap_or(subscription);
    let applied = if quota_is_newer(&account.quota, &initial.quota) {
        let credits_updated = match &quota {
            QuotaRefreshOutcome::Updated(result) => result
                .quota
                .clone()
                .normalize(&account.quota)
                .is_ok_and(|(snapshot, _)| account.quota.merge_provider_credits(&snapshot)),
            QuotaRefreshOutcome::Failed { .. } => false,
        };
        let outcome = if credits_updated {
            QuotaReadOutcome::Updated {
                transitions: Vec::new(),
                exhaustion_transitions: Vec::new(),
            }
        } else {
            QuotaReadOutcome::Skipped
        };
        (outcome, Vec::new())
    } else {
        apply_quota_outcome(account, quota, started_at_ms)
    };
    account.subscription = if before.subscription != initial.subscription
        || (!partial
            && (subscription.updated_at_ms.is_none()
                || subscription.updated_at_ms < initial.subscription.updated_at_ms))
    {
        before.subscription
    } else {
        subscription
    };
    applied
}

pub fn preserve_newer_account_state(
    account: &mut AccountRecord,
    before_refresh: &AccountRecord,
    latest: &AccountRecord,
) {
    if latest.auth_state != before_refresh.auth_state {
        account.auth_state = latest.auth_state;
    }
    if latest.health != before_refresh.health {
        account.health = latest.health;
    }
    if latest.last_error_code != before_refresh.last_error_code {
        account.last_error_code = latest.last_error_code.clone();
    }
}

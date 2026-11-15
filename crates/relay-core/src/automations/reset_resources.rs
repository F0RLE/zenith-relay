//! Optional reset capabilities are distinct from ordinary quota and admission.
use super::{WakeTask, WakeTrigger};
use crate::accounts::{
    AccountAuthState, AccountHealthState, AccountRouteGroup, management::ManagedAccount,
};
use crate::quota::{QuotaWindow, QuotaWindowKind, Subscription, SubscriptionStatus};
use std::collections::BTreeMap;

pub const THRESHOLD_PREFIX: &str = "weekly-threshold:";
pub const PENDING_SUFFIX: &str = ":pending";

pub fn registration_eligible<A: ManagedAccount>(record: &A) -> bool {
    let account = record.account();
    !record.requires_recovery()
        && account.enabled
        && !account.draining
        && account.auth_state == AccountAuthState::Active
        && (account.health == AccountHealthState::Healthy
            || (account.health == AccountHealthState::Degraded
                && account.last_error_code.as_deref() == Some("upstream_route_degraded")))
}

pub fn pool_member_eligible<A: ManagedAccount>(record: &A) -> bool {
    record.account().in_pool && registration_eligible(record)
}

pub fn selected_pool_account<'a, A: ManagedAccount>(
    groups: &[AccountRouteGroup],
    accounts: &'a [A],
    reader: &A,
) -> Option<&'a A> {
    if !registration_eligible(reader) {
        return None;
    }
    let selected_id = groups
        .iter()
        .find(|g| g.account_ids.contains(&reader.account().id))
        .map_or(&reader.account().id, |g| &g.selected_account_id);
    accounts
        .iter()
        .find(|record| record.account().id == *selected_id && pool_member_eligible(*record))
}

pub fn rule_matches<A: ManagedAccount>(task: &WakeTask, reader: &A, selected: &A) -> bool {
    task.enabled
        && task.trigger == WakeTrigger::Weekly
        && (task.account_selector.matches(reader.account())
            || task.account_selector.matches(selected.account()))
}

/// Capability evidence is supplied by the owning connector, never inferred from quota.
pub fn reset_member<'a, A: ManagedAccount>(
    groups: &[AccountRouteGroup],
    accounts: &'a [A],
    reader: &A,
    can_reset: impl Fn(&str) -> bool,
) -> Option<&'a A> {
    let selected = selected_pool_account(groups, accounts, reader)?;
    let Some(group) = groups
        .iter()
        .find(|g| g.account_ids.contains(&reader.account().id))
    else {
        return can_reset(&reader.account().id).then_some(selected);
    };
    let owner = accounts
        .iter()
        .filter(|candidate| {
            group.account_ids.contains(&candidate.account().id)
                && can_reset(&candidate.account().id)
                && selected_pool_account(groups, accounts, *candidate).is_some()
        })
        .min_by_key(|candidate| {
            (
                candidate.account().id != selected.account().id,
                &candidate.account().id,
            )
        })?;
    (owner.account().id == reader.account().id).then_some(selected)
}

pub fn linked_attempts<'a>(
    groups: &'a [AccountRouteGroup],
    fingerprints: &'a BTreeMap<String, String>,
    account_id: &'a str,
) -> impl Iterator<Item = &'a str> {
    let group = groups
        .iter()
        .find(|g| g.account_ids.iter().any(|id| id == account_id));
    fingerprints.iter().filter_map(move |(id, marker)| {
        (id == account_id || group.is_some_and(|g| g.account_ids.contains(id)))
            .then_some(marker.as_str())
    })
}

pub fn subscription_allows_reset(subscription: &Subscription, now_ms: u64) -> bool {
    !matches!(
        subscription.status,
        SubscriptionStatus::Expired | SubscriptionStatus::Forbidden
    ) && subscription
        .active_until_ms
        .is_none_or(|expiry| expiry > now_ms)
}

pub fn threshold_window<'a, A: ManagedAccount>(
    reader: &'a A,
    selected: &A,
    tasks: &[WakeTask],
    updated: bool,
    now_ms: u64,
) -> Option<&'a QuotaWindow> {
    let account = reader.account();
    if !updated
        || reader.requires_recovery()
        || !pool_member_eligible(selected)
        || !subscription_allows_reset(&account.subscription, now_ms)
        || account.quota.reset_credits_available == Some(0)
    {
        return None;
    }
    let window = account.quota.secondary.as_ref()?;
    let remaining = window.available_basis_points?;
    if window.kind != QuotaWindowKind::Secondary
        || remaining >= 10_000
        || window.explicitly_full == Some(true)
        || window
            .window_minutes
            .is_some_and(|minutes| minutes != 10_080)
        || window.reset_at_ms.is_none_or(|reset| reset <= now_ms)
        || !now_ms
            .checked_sub(window.observed_at_ms)
            .is_some_and(|age| age <= crate::scheduler::QUOTA_STALE_AFTER_MS)
    {
        return None;
    }
    tasks
        .iter()
        .any(|task| {
            rule_matches(task, reader, selected)
                && task
                    .weekly_reset_remaining_basis_points
                    .is_some_and(|threshold| threshold <= 10_000 && remaining <= threshold)
        })
        .then_some(window)
}

pub fn has_pending_redemption(
    groups: &[AccountRouteGroup],
    fingerprints: &BTreeMap<String, String>,
    account_id: &str,
) -> bool {
    linked_attempts(groups, fingerprints, account_id)
        .any(|v| v.starts_with(THRESHOLD_PREFIX) && v.ends_with(PENDING_SUFFIX))
}

pub fn already_attempted(
    groups: &[AccountRouteGroup],
    fingerprints: &BTreeMap<String, String>,
    account_id: &str,
    now_ms: u64,
) -> bool {
    linked_attempts(groups, fingerprints, account_id).any(|value| {
        value.strip_prefix(THRESHOLD_PREFIX).is_some_and(|value| {
            value.ends_with(PENDING_SUFFIX)
                || value
                    .split(':')
                    .next()
                    .and_then(|at| at.parse::<u64>().ok())
                    .is_some_and(|at| at > now_ms)
        })
    })
}

pub fn has_pending_confirmation<A: ManagedAccount>(
    groups: &[AccountRouteGroup],
    fingerprints: &BTreeMap<String, String>,
    accounts: &[A],
    account_id: &str,
) -> bool {
    let group = groups
        .iter()
        .find(|g| g.account_ids.iter().any(|id| id == account_id));
    has_pending_redemption(groups, fingerprints, account_id)
        || accounts.iter().any(|record| {
            let account = record.account();
            (account.id == account_id || group.is_some_and(|g| g.account_ids.contains(&account.id)))
                && !record.requires_recovery()
                && (account.in_pool
                    || groups
                        .iter()
                        .find(|g| g.account_ids.contains(&account.id))
                        .is_some_and(|g| {
                            accounts.iter().any(|a| {
                                a.account().id == g.selected_account_id && a.account().in_pool
                            })
                        }))
                && [
                    account.quota.primary.as_ref(),
                    account.quota.secondary.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|window| {
                    window.available_basis_points == Some(0)
                        && window
                            .exhaustion_transition_fingerprint
                            .as_ref()
                            .is_some_and(|id| fingerprints.get(&account.id) == Some(id))
                })
        })
}

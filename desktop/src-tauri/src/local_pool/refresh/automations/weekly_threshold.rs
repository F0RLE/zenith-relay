use super::*;
use crate::local_pool::accounts::pool_resources::weekly_reset_pool_member_eligible;
use crate::local_pool::models::AutomationRecords;
use zenith_relay_core::quota::QuotaWindow;

const PREFIX: &str = "weekly-threshold:";
const PENDING: &str = ":pending";

pub(super) fn subscription_allows_reset(
    subscription: &zenith_relay_core::quota::Subscription,
    now_ms: u64,
) -> bool {
    !matches!(
        subscription.status,
        zenith_relay_core::quota::SubscriptionStatus::Expired
            | zenith_relay_core::quota::SubscriptionStatus::Forbidden
    ) && subscription
        .active_until_ms
        .is_none_or(|expiry| expiry > now_ms)
}

pub(super) fn has_matching_rule(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
) -> Result<bool> {
    let store = state.store()?;
    let credentials = state.cached_account_credential_facts(store.accounts())?;
    let Some(selected) = weekly_reset_member(
        store.gateway(),
        store.accounts(),
        &response.account,
        &credentials,
    ) else {
        return Ok(false);
    };
    Ok(store.automations().tasks.iter().any(|task| {
        weekly_reset_rule_matches(task, &response.account, selected)
            && task.weekly_reset_remaining_basis_points.is_some()
    }))
}

fn due_window<'a>(
    response: &'a AccountQuotaRefreshResponse,
    records: &AutomationRecords,
    selected: &crate::local_pool::models::LocalAccountRecord,
    now_ms: u64,
) -> Option<&'a QuotaWindow> {
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. })
        || response.account.recovery.is_some()
        || !weekly_reset_pool_member_eligible(selected)
        || !subscription_allows_reset(&response.account.account.subscription, now_ms)
        || response.account.account.quota.reset_credits_available == Some(0)
    {
        return None;
    }
    let window = response.account.account.quota.secondary.as_ref()?;
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
            .is_some_and(|age| age <= zenith_relay_core::scheduler::QUOTA_STALE_AFTER_MS)
    {
        return None;
    }
    records
        .tasks
        .iter()
        .any(|task| {
            weekly_reset_rule_matches(task, &response.account, selected)
                && task
                    .weekly_reset_remaining_basis_points
                    .is_some_and(|threshold| threshold <= 10_000 && remaining <= threshold)
        })
        .then_some(window)
}

pub(super) fn has_pending_redemption(
    records: &AutomationRecords,
    settings: &crate::local_pool::models::GatewaySettings,
    account_id: &str,
) -> bool {
    crate::local_pool::accounts::pool_resources::linked_reset_attempts(
        settings, records, account_id,
    )
    .any(|value| value.starts_with(PREFIX) && value.ends_with(PENDING))
}

pub(super) fn has_pending_confirmation(
    records: &AutomationRecords,
    accounts: &[crate::local_pool::models::LocalAccountRecord],
    settings: &crate::local_pool::models::GatewaySettings,
    account_id: &str,
) -> bool {
    let group = settings.account_route_group(account_id);
    has_pending_redemption(records, settings, account_id)
        || accounts.iter().any(|account| {
            (account.account.id == account_id
                || group.is_some_and(|group| group.account_ids.contains(&account.account.id)))
                && account.recovery.is_none()
                && (account.account.in_pool
                    || settings
                        .account_route_group(&account.account.id)
                        .is_some_and(|group| {
                            accounts.iter().any(|selected| {
                                selected.account.id == group.selected_account_id
                                    && selected.account.in_pool
                            })
                        }))
                && [
                    account.account.quota.primary.as_ref(),
                    account.account.quota.secondary.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|window| {
                    window.available_basis_points == Some(0)
                        && window
                            .exhaustion_transition_fingerprint
                            .as_ref()
                            .is_some_and(|id| {
                                records.weekly_reset_fingerprints.get(&account.account.id)
                                    == Some(id)
                            })
                })
        })
}

fn already_attempted(
    records: &AutomationRecords,
    settings: &crate::local_pool::models::GatewaySettings,
    account_id: &str,
    now_ms: u64,
) -> bool {
    crate::local_pool::accounts::pool_resources::linked_reset_attempts(
        settings, records, account_id,
    )
    .any(|value| {
        value.strip_prefix(PREFIX).is_some_and(|value| {
            value.ends_with(PENDING)
                || value
                    .split(':')
                    .next()
                    .and_then(|deadline| deadline.parse::<u64>().ok())
                    .is_some_and(|deadline| deadline > now_ms)
        })
    })
}

/// Only a fresh complete weekly ledger confirms an ambiguous redemption.
pub(super) fn confirm_redemption(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
) -> Result<()> {
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. })
        || response
            .account
            .account
            .quota
            .secondary
            .as_ref()
            .is_none_or(|window| {
                !window.is_fully_available()
                    || !current_time_ms()
                        .checked_sub(window.observed_at_ms)
                        .is_some_and(|age| {
                            age <= zenith_relay_core::scheduler::QUOTA_STALE_AFTER_MS
                        })
            })
    {
        return Ok(());
    }
    let mut store = state.store()?;
    store.ensure_account_refresh_current(fence)?;
    if store
        .account(&fence.account_id)
        .is_none_or(|account| account.account.quota != response.account.account.quota)
    {
        return Ok(());
    }
    let mut records = store.automations().clone();
    let Some(marker) = records.weekly_reset_fingerprints.get_mut(&fence.account_id) else {
        return Ok(());
    };
    if marker.starts_with(PREFIX) && marker.ends_with(PENDING) {
        marker.truncate(marker.len() - PENDING.len());
        store.replace_automations(records)?;
    }
    Ok(())
}

pub(super) async fn evaluate(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
) -> Result<Option<AccountQuotaRefreshResponse>> {
    let _reset = state.reset_redemption_guard().await;
    let now_ms = current_time_ms();
    let (fingerprint, member_fence) = {
        let store = state.store()?;
        store.ensure_account_refresh_current(fence)?;
        let credentials = state.cached_account_credential_facts(store.accounts())?;
        let Some(selected) = weekly_reset_member(
            store.gateway(),
            store.accounts(),
            &response.account,
            &credentials,
        ) else {
            return Ok(None);
        };
        let Some(window) = due_window(response, store.automations(), selected, now_ms) else {
            return Ok(None);
        };
        if has_pending_redemption(store.automations(), store.gateway(), &fence.account_id)
            || already_attempted(
                store.automations(),
                store.gateway(),
                &fence.account_id,
                now_ms,
            )
        {
            return Ok(None);
        }
        let (_, member_fence) = store.account_refresh_scope(&selected.account.id)?;
        (
            format!(
                "{PREFIX}{}{PENDING}",
                window.reset_at_ms.expect("due window has a deadline")
            ),
            member_fence,
        )
    };
    let member_fence = &member_fence;
    let redeemed =
        consume_reset_credit_when(state, fence, Some(&fingerprint), |expiry| async move {
            if expiry.is_some_and(|expiry| expiry <= current_time_ms()) {
                return Ok(false);
            }
            let store = state.store()?;
            store.ensure_account_refresh_current(fence)?;
            store.ensure_account_refresh_current(member_fence)?;
            let Some(account) = store.account(&fence.account_id) else {
                return Ok(false);
            };
            let current = account.account.quota == response.account.account.quota;
            let credentials = state.cached_account_credential_facts(store.accounts())?;
            let Some(selected) =
                weekly_reset_member(store.gateway(), store.accounts(), account, &credentials)
            else {
                return Ok(false);
            };
            let mut current_response = response.clone();
            current_response.account = account.clone();
            Ok(current
                && selected.account.id == member_fence.account_id
                && !has_pending_confirmation(
                    store.automations(),
                    store.accounts(),
                    store.gateway(),
                    &fence.account_id,
                )
                && !already_attempted(
                    store.automations(),
                    store.gateway(),
                    &fence.account_id,
                    current_time_ms(),
                )
                && due_window(
                    &current_response,
                    store.automations(),
                    selected,
                    current_time_ms(),
                )
                .is_some())
        })
        .await;
    if !redeemed.is_ok_and(|redeemed| redeemed) {
        return Ok(None);
    }
    let scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
    if scope.fence != *fence {
        return Ok(None);
    }
    let refreshed =
        crate::local_pool::accounts::quota_refresh::read_account_quota_once(state, &scope, false)
            .await?;
    confirm_redemption(state, &refreshed, fence)?;
    Ok(Some(refreshed))
}

#[cfg(test)]
mod tests;

use super::*;
use crate::local_pool::models::AutomationRecords;

use zenith_relay_core::automations::reset_resources::{
    PENDING_SUFFIX as PENDING, THRESHOLD_PREFIX as PREFIX,
};

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
    zenith_relay_core::automations::reset_resources::threshold_window(
        &response.account,
        selected,
        &records.tasks,
        matches!(response.quota, AccountQuotaOutcome::Updated { .. }),
        now_ms,
    )
}

#[cfg(test)]
fn has_pending_redemption(
    records: &AutomationRecords,
    settings: &crate::local_pool::models::GatewaySettings,
    account_id: &str,
) -> bool {
    zenith_relay_core::automations::reset_resources::has_pending_redemption(
        &settings.account_routes,
        &records.weekly_reset_fingerprints,
        account_id,
    )
}

pub(super) fn has_pending_confirmation(
    records: &AutomationRecords,
    accounts: &[crate::local_pool::models::LocalAccountRecord],
    settings: &crate::local_pool::models::GatewaySettings,
    account_id: &str,
) -> bool {
    zenith_relay_core::automations::reset_resources::has_pending_confirmation(
        &settings.account_routes,
        &records.weekly_reset_fingerprints,
        accounts,
        account_id,
    )
}

fn already_attempted(
    records: &AutomationRecords,
    settings: &crate::local_pool::models::GatewaySettings,
    account_id: &str,
    now_ms: u64,
) -> bool {
    zenith_relay_core::automations::reset_resources::already_attempted(
        &settings.account_routes,
        &records.weekly_reset_fingerprints,
        account_id,
        now_ms,
    )
}

/// Only a fresh complete weekly ledger confirms an ambiguous redemption.
pub(super) fn confirm_redemption(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
) -> Result<()> {
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. })
        || fresh_weekly_reset_window(&response.account.account.quota, current_time_ms()).is_none_or(
            |window| {
                window.available_basis_points != Some(10_000)
                    || window.explicitly_full == Some(false)
            },
        )
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
        if has_pending_confirmation(
            store.automations(),
            store.accounts(),
            store.gateway(),
            &fence.account_id,
        ) || already_attempted(
            store.automations(),
            store.gateway(),
            &fence.account_id,
            now_ms,
        ) {
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
    let attempt = ResetAttempt::new(ResetOrigin::WeeklyThreshold(
        &response.account.account.quota,
    ));
    let redeemed = consume_reset_credit_when(
        state,
        fence,
        Some(&fingerprint),
        &attempt,
        |expiry| async move {
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
        },
    )
    .await;
    if !redeemed.is_ok_and(|redeemed| redeemed) {
        return Ok(None);
    }
    let Some(refreshed) = verify_automatic_weekly_reset(state, fence, &attempt).await? else {
        return Ok(None);
    };
    confirm_redemption(state, &refreshed, fence)?;
    Ok(Some(refreshed))
}

#[cfg(test)]
mod tests;

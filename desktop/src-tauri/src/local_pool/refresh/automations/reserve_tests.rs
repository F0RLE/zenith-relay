use super::*;
use crate::local_pool::refresh::tests::{account, cleanup, state};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    automations::{AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTrigger},
};

fn weekly_reset_still_current(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
) -> Result<bool> {
    super::weekly_reset_still_current(state, response, fence, &response.account.account.id)
}

pub(super) fn exhausted_response(state: &DesktopState) -> AccountQuotaRefreshResponse {
    let now_ms = current_time_ms();
    let mut account = account();
    account.account.in_pool = true;
    account.account.auth_state = AccountAuthState::Active;
    account.account.health = AccountHealthState::Healthy;
    account.account.quota.reset_credits_available = Some(1);
    account.account.quota.updated_at_ms = Some(now_ms);
    account.account.quota.secondary = Some(QuotaWindow {
        kind: QuotaWindowKind::Secondary,
        provider_cycle_id: Some("synthetic-week".into()),
        window_start_ms: Some(now_ms.saturating_sub(1_000)),
        available_basis_points: Some(0),
        explicitly_full: Some(false),
        reset_at_ms: Some(now_ms + 86_400_000),
        window_minutes: Some(10_080),
        observed_at_ms: now_ms,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: Some("synthetic-exhaustion".into()),
    });
    let mut store = state.store().unwrap();
    store.upsert_account(account.clone()).unwrap();
    let mut records = store.automations().clone();
    records.tasks.push(WakeTask {
        id: "synthetic-weekly".into(),
        name: "Synthetic weekly reserve".into(),
        enabled: true,
        account_selector: AccountSelector::AllEligible,
        window_kinds: [QuotaWindowKind::Secondary].into(),
        model_policy: WakeModelPolicy::LightestSupported,
        trigger: WakeTrigger::Weekly,
        weekly_reset_remaining_basis_points: None,
        fallback_schedule: None,
        execution_policy: WakeExecutionPolicy::Automatic,
        jitter_seconds: 0,
        max_attempts_per_cycle: 1,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    });
    store.replace_automations(records).unwrap();
    AccountQuotaRefreshResponse {
        account,
        quota: AccountQuotaOutcome::Updated {
            transitions: vec![],
            exhaustion_transitions: vec![],
        },
        exhaustion_transitions: vec![],
    }
}

#[tokio::test]
async fn linked_reader_uses_selected_members_rule_without_joining_the_pool() {
    use crate::local_pool::models::AccountRouteGroup;
    let state = state();
    let mut response = exhausted_response(&state);
    response.account.account.in_pool = false;
    let reader_id = response.account.account.id.clone();
    let mut selected = account();
    selected.account.id = "synthetic-selected".into();
    selected.account.in_pool = true;
    let selected_id = selected.account.id.clone();
    {
        let mut store = state.store().unwrap();
        store.upsert_account(response.account.clone()).unwrap();
        store.upsert_account(selected.clone()).unwrap();
        let mut gateway = store.gateway().clone();
        gateway.account_routes.push(AccountRouteGroup {
            account_ids: vec![reader_id.clone(), selected_id.clone()],
            selected_account_id: selected_id.clone(),
            notes_account_id: None,
        });
        store.replace_gateway(gateway).unwrap();
        let mut automations = store.automations().clone();
        automations.tasks[0].account_selector =
            AccountSelector::AccountIds([selected_id.clone()].into());
        store.replace_automations(automations).unwrap();
    }
    let (_, fence) = state
        .store()
        .unwrap()
        .account_refresh_scope(&reader_id)
        .unwrap();
    let credentials = crate::local_pool::accounts::pool_resources::tests::native_facts(
        std::slice::from_ref(&response.account),
    );
    assert!(weekly_reset_current(
        &state.store().unwrap(),
        &response,
        &fence,
        &selected_id,
        &credentials,
        current_time_ms()
    ));
    assert!(!weekly_reset_current(
        &state.store().unwrap(),
        &response,
        &fence,
        "previous-selected",
        &credentials,
        current_time_ms()
    ));
    assert!(
        !state
            .store()
            .unwrap()
            .account(&reader_id)
            .unwrap()
            .account
            .in_pool
    );
    selected.account.enabled = false;
    state.store().unwrap().upsert_account(selected).unwrap();
    assert!(!weekly_reset_current(
        &state.store().unwrap(),
        &response,
        &fence,
        &selected_id,
        &credentials,
        current_time_ms()
    ));
    cleanup(state).await;
}

#[tokio::test]
async fn weekly_reserve_rechecks_entitlement_expiry_before_redemption() {
    use zenith_relay_core::quota::{Subscription, SubscriptionStatus};
    let state = state();
    let mut response = exhausted_response(&state);
    let now_ms = current_time_ms();
    for (deadline, status, eligible) in [
        (Some(now_ms + 60_000), SubscriptionStatus::Active, true),
        (None, SubscriptionStatus::Unknown, true),
        (Some(now_ms - 1), SubscriptionStatus::Active, false),
        (Some(now_ms + 60_000), SubscriptionStatus::Expired, false),
        (Some(now_ms + 60_000), SubscriptionStatus::Forbidden, false),
    ] {
        response.account.account.subscription = Subscription {
            plan_type: Some("plus".into()),
            active_until_ms: deadline,
            status,
            updated_at_ms: Some(now_ms),
        };
        state
            .store()
            .unwrap()
            .upsert_account(response.account.clone())
            .unwrap();
        let (_, fence) = state
            .store()
            .unwrap()
            .account_refresh_scope(&response.account.account.id)
            .unwrap();
        assert_eq!(
            weekly_reset_still_current(&state, &response, &fence).unwrap(),
            eligible
        );
    }
    cleanup(state).await;
}

#[tokio::test]
async fn weekly_reserve_accepts_route_degradation_but_preserves_health_and_rule_gates() {
    let state = state();
    let mut response = exhausted_response(&state);
    response.account.account.health = AccountHealthState::Degraded;
    for code in [
        Some("upstream_route_degraded"),
        Some("quota_refresh_failed"),
        None,
    ] {
        response.account.account.last_error_code = code.map(str::to_owned);
        let (_, fence) = {
            let mut store = state.store().unwrap();
            store.upsert_account(response.account.clone()).unwrap();
            store
                .account_refresh_scope(&response.account.account.id)
                .unwrap()
        };
        assert_eq!(
            weekly_reset_still_current(&state, &response, &fence).unwrap(),
            code == Some("upstream_route_degraded")
        );
    }
    response.account.account.last_error_code = Some("upstream_route_degraded".into());
    let (_, fence) = {
        let mut store = state.store().unwrap();
        store.upsert_account(response.account.clone()).unwrap();
        let mut records = store.automations().clone();
        records.tasks[0].enabled = false;
        store.replace_automations(records).unwrap();
        store
            .account_refresh_scope(&response.account.account.id)
            .unwrap()
    };
    assert!(!weekly_reset_still_current(&state, &response, &fence).unwrap());
    cleanup(state).await;
}

#[tokio::test]
async fn weekly_reserve_rechecks_current_rule_and_account_observation() {
    let state = state();
    let response = exhausted_response(&state);
    let (_, fence) = state
        .store()
        .unwrap()
        .account_refresh_scope(&response.account.account.id)
        .unwrap();
    assert!(weekly_reset_still_current(&state, &response, &fence).unwrap());
    let mut records = state.store().unwrap().automations().clone();
    let mut threshold = records.tasks[0].clone();
    threshold.id = "synthetic-threshold".into();
    threshold.weekly_reset_remaining_basis_points = Some(1_000);
    records.tasks.push(threshold);
    state
        .store()
        .unwrap()
        .replace_automations(records.clone())
        .unwrap();
    assert!(!weekly_reset_still_current(&state, &response, &fence).unwrap());
    records.tasks.pop();
    records.tasks[0].enabled = false;
    state.store().unwrap().replace_automations(records).unwrap();
    assert!(!weekly_reset_still_current(&state, &response, &fence).unwrap());
    assert!(
        evaluate_weekly_exhaustions(&state, &response, &fence)
            .await
            .unwrap()
            .is_none()
    );
    cleanup(state).await;
}

#[tokio::test]
async fn weekly_reserve_never_acts_on_a_failed_or_skipped_quota_read() {
    let state = state();
    let mut response = exhausted_response(&state);
    let (_, fence) = state
        .store()
        .unwrap()
        .account_refresh_scope(&response.account.account.id)
        .unwrap();
    for outcome in [
        AccountQuotaOutcome::Skipped,
        AccountQuotaOutcome::Failed {
            code: "synthetic-quota-failure".into(),
            retryable: true,
        },
    ] {
        response.quota = outcome;
        assert!(weekly_exhaustion_fingerprint(&response, current_time_ms()).is_none());
        assert!(!weekly_reset_still_current(&state, &response, &fence).unwrap());
        assert!(
            evaluate_weekly_exhaustions(&state, &response, &fence)
                .await
                .unwrap()
                .is_none()
        );
    }
    cleanup(state).await;
}

#[tokio::test]
async fn an_unconfirmed_redemption_survives_restart_and_blocks_another_auto_reset() {
    let state = state();
    let response = exhausted_response(&state);
    let id = response.account.account.id.clone();
    let (_, fence) = state.store().unwrap().account_refresh_scope(&id).unwrap();
    assert!(weekly_reset_still_current(&state, &response, &fence).unwrap());
    state
        .mark_weekly_reset_applied(&id, "synthetic-exhaustion")
        .unwrap();
    assert!(!weekly_reset_still_current(&state, &response, &fence).unwrap());
    let root = state.root.clone();
    state.refresh.shutdown().await;
    drop(state);
    let reopened = DesktopState::open(root).unwrap();
    let (_, fence) = reopened
        .store()
        .unwrap()
        .account_refresh_scope(&id)
        .unwrap();
    assert!(!weekly_reset_still_current(&reopened, &response, &fence).unwrap());
    cleanup(reopened).await;
}

#[tokio::test]
async fn reset_coordination_keeps_other_redemptions_waiting_until_verification() {
    let state = state();
    let held = state.reset_redemption_guard().await;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            state.reset_redemption_guard()
        )
        .await
        .is_err()
    );
    drop(held);
    let next = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        state.reset_redemption_guard(),
    )
    .await
    .unwrap();
    drop(next);
    cleanup(state).await;
}

#[tokio::test]
async fn quota_exhaustion_wakes_only_other_enabled_weekly_reserves() {
    let state = state();
    let mut response = exhausted_response(&state);
    let mut other = account();
    other.account.in_pool = true;
    other.account.auth_state = AccountAuthState::Active;
    other.account.health = AccountHealthState::Healthy;
    other.account.quota = response.account.account.quota.clone();
    let id = other.account.id.clone();
    state
        .store()
        .unwrap()
        .upsert_account(other.clone())
        .unwrap();
    assert!(
        weekly_reserve_targets(&state, &response)
            .unwrap()
            .is_empty()
    );
    response.exhaustion_transitions.push(QuotaTransition {
        window_kind: QuotaWindowKind::Primary,
        fingerprint: "synthetic-primary-exhaustion".into(),
        transitioned_at_ms: current_time_ms(),
    });
    let targets = weekly_reserve_targets(&state, &response).unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].member_id,
        zenith_relay_core::scheduler::account_member_key(&id)
    );
    let mut records = state.store().unwrap().automations().clone();
    records.tasks[0].weekly_reset_remaining_basis_points = Some(1_000);
    state
        .store()
        .unwrap()
        .replace_automations(records.clone())
        .unwrap();
    assert!(
        weekly_reserve_targets(&state, &response)
            .unwrap()
            .is_empty()
    );
    records.tasks[0].weekly_reset_remaining_basis_points = None;
    state.store().unwrap().replace_automations(records).unwrap();
    other.account.quota.reset_credits_available = Some(0);
    state.store().unwrap().upsert_account(other).unwrap();
    assert!(
        weekly_reserve_targets(&state, &response)
            .unwrap()
            .is_empty()
    );
    cleanup(state).await;
}

#[tokio::test]
async fn unconfirmed_manual_reset_blocks_only_its_account_and_linked_ledger() {
    let state = state();
    let response = exhausted_response(&state);
    let (_, fence) = state
        .store()
        .unwrap()
        .account_refresh_scope(&response.account.account.id)
        .unwrap();
    let mut other = account();
    other.account.in_pool = true;
    other.account.auth_state = AccountAuthState::Active;
    other.account.health = AccountHealthState::Healthy;
    let mut primary = response.account.account.quota.secondary.clone().unwrap();
    primary.kind = QuotaWindowKind::Primary;
    primary.exhaustion_transition_fingerprint = Some("synthetic-manual-primary".into());
    other.account.quota.primary = Some(primary);
    let id = other.account.id.clone();
    state
        .store()
        .unwrap()
        .upsert_account(other.clone())
        .unwrap();
    state
        .mark_weekly_reset_applied(&id, "synthetic-manual-primary")
        .unwrap();
    assert!(weekly_reset_still_current(&state, &response, &fence).unwrap());
    let records = state.store().unwrap().automations().clone();
    let mut settings = state.store().unwrap().gateway().clone();
    let accounts = [response.account.clone(), other.clone()];
    assert!(weekly_threshold::has_pending_confirmation(
        &records, &accounts, &settings, &id,
    ));
    settings
        .account_routes
        .push(crate::local_pool::models::AccountRouteGroup {
            account_ids: vec![response.account.account.id.clone(), id.clone()],
            selected_account_id: response.account.account.id.clone(),
            notes_account_id: None,
        });
    assert!(weekly_threshold::has_pending_confirmation(
        &records,
        &accounts,
        &settings,
        &response.account.account.id,
    ));
    other
        .account
        .quota
        .primary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(10_000);
    state
        .store()
        .unwrap()
        .upsert_account(other.clone())
        .unwrap();
    assert!(!weekly_threshold::has_pending_confirmation(
        &records,
        &[response.account.clone(), other],
        &settings,
        &response.account.account.id,
    ));
    assert!(weekly_reset_still_current(&state, &response, &fence).unwrap());
    cleanup(state).await;
}

#[tokio::test]
async fn stale_future_or_nonempty_weekly_observations_never_spend_a_reset() {
    let state = state();
    let mut response = exhausted_response(&state);
    let now = current_time_ms();
    assert!(fresh_weekly_exhaustion(&response, now).is_some());
    for (remaining, at) in [
        (
            Some(0),
            now - zenith_relay_core::scheduler::QUOTA_STALE_AFTER_MS - 1,
        ),
        (Some(0), now + 60_000),
        (Some(1), now),
        (None, now),
    ] {
        let window = response.account.account.quota.secondary.as_mut().unwrap();
        window.available_basis_points = remaining;
        window.observed_at_ms = at;
        assert!(fresh_weekly_exhaustion(&response, now).is_none());
    }
    cleanup(state).await;
}

#[tokio::test]
async fn five_hour_exhaustion_does_not_redeem_without_a_proven_exhausted_week() {
    let state = state();
    let mut response = exhausted_response(&state);
    let now = current_time_ms();
    let quota = &mut response.account.account.quota;
    let weekly = quota.secondary.clone().unwrap();
    quota.primary = Some(QuotaWindow {
        kind: QuotaWindowKind::Primary,
        window_minutes: Some(300),
        reset_at_ms: Some(now + 300_000),
        ..weekly.clone()
    });
    for window in [
        None,
        Some(QuotaWindow {
            available_basis_points: Some(4_700),
            ..weekly.clone()
        }),
        Some(QuotaWindow {
            window_minutes: Some(300),
            ..weekly.clone()
        }),
        Some(QuotaWindow {
            window_minutes: None,
            ..weekly.clone()
        }),
        Some(QuotaWindow {
            reset_at_ms: Some(now),
            ..weekly.clone()
        }),
        Some(QuotaWindow {
            reset_at_ms: None,
            ..weekly.clone()
        }),
        Some(QuotaWindow {
            explicitly_full: Some(true),
            ..weekly.clone()
        }),
    ] {
        response.account.account.quota.secondary = window;
        let (_, fence) = {
            let mut store = state.store().unwrap();
            store.upsert_account(response.account.clone()).unwrap();
            store
                .account_refresh_scope(&response.account.account.id)
                .unwrap()
        };
        assert!(weekly_exhaustion_fingerprint(&response, now).is_none());
        assert!(!weekly_reset_still_current(&state, &response, &fence).unwrap());
    }
    cleanup(state).await;
}

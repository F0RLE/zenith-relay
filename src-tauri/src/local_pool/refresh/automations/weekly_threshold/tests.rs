use super::*;
use crate::local_pool::refresh::{
    automations::reserve_tests::exhausted_response,
    tests::{cleanup, state},
};

#[tokio::test]
async fn threshold_is_opt_in_and_only_uses_fresh_general_weekly_quota() {
    let state = state();
    let mut response = exhausted_response(&state);
    let now = current_time_ms();
    let mut records = state.store().unwrap().automations().clone();
    assert!(due_window(&response, &records, now).is_none());
    records.tasks[0].weekly_reset_remaining_basis_points = Some(2_000);
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(2_000);
    assert!(due_window(&response, &records, now).is_some());
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(2_001);
    assert!(due_window(&response, &records, now).is_none());
    records.tasks[0].weekly_reset_remaining_basis_points = Some(10_000);
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(10_000);
    assert!(due_window(&response, &records, now).is_none());
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(9_999);
    assert!(due_window(&response, &records, now).is_some());
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .window_minutes = Some(300);
    assert!(due_window(&response, &records, now).is_none());
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .window_minutes = Some(10_080);
    assert!(
        due_window(
            &response,
            &records,
            now + zenith_relay_core::scheduler::QUOTA_STALE_AFTER_MS + 1
        )
        .is_none()
    );
    response.quota = AccountQuotaOutcome::Skipped;
    assert!(due_window(&response, &records, now).is_none());
    cleanup(state).await;
}

#[tokio::test]
async fn zero_threshold_requires_exhaustion_and_a_known_cycle() {
    let state = state();
    let mut response = exhausted_response(&state);
    let now = current_time_ms();
    let mut records = state.store().unwrap().automations().clone();
    records.tasks[0].weekly_reset_remaining_basis_points = Some(0);
    assert!(due_window(&response, &records, now).is_some());
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(1);
    assert!(due_window(&response, &records, now).is_none());
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(0);
    response
        .account
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .reset_at_ms = None;
    assert!(due_window(&response, &records, now).is_none());
    cleanup(state).await;
}

#[tokio::test]
async fn ambiguous_threshold_redemption_is_persisted_until_fresh_full_confirmation() {
    let original = state();
    let mut response = exhausted_response(&original);
    let id = response.account.account.id.clone();
    let now = current_time_ms();
    let deadline = now + 86_400_000;
    let marker = format!("{PREFIX}{deadline}{PENDING}");
    original.mark_weekly_reset_applied(&id, &marker).unwrap();
    let root = original.root.clone();
    original.refresh.shutdown().await;
    drop(original);
    let state = DesktopState::open(root).unwrap();
    let (_, fence) = state.store().unwrap().account_refresh_scope(&id).unwrap();
    confirm_redemption(&state, &response, &fence).unwrap();
    assert!(has_pending_redemption(state.store().unwrap().automations()));
    assert!(already_attempted(
        state.store().unwrap().automations(),
        &id,
        deadline + 1
    ));
    let window = response.account.account.quota.secondary.as_mut().unwrap();
    window.available_basis_points = Some(10_000);
    window.explicitly_full = Some(true);
    state
        .store()
        .unwrap()
        .upsert_account(response.account.clone())
        .unwrap();
    confirm_redemption(&state, &response, &fence).unwrap();
    assert!(!has_pending_redemption(
        state.store().unwrap().automations()
    ));
    assert!(already_attempted(
        state.store().unwrap().automations(),
        &id,
        now
    ));
    assert!(!already_attempted(
        state.store().unwrap().automations(),
        &id,
        deadline
    ));
    cleanup(state).await;
}

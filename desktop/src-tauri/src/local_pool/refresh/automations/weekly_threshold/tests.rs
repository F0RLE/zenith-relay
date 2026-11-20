use super::*;

fn already_attempted(records: &AutomationRecords, account_id: &str, now_ms: u64) -> bool {
    super::already_attempted(records, &Default::default(), account_id, now_ms)
}

fn due_window<'a>(
    response: &'a AccountQuotaRefreshResponse,
    records: &AutomationRecords,
    now_ms: u64,
) -> Option<&'a QuotaWindow> {
    super::due_window(response, records, &response.account, now_ms)
}
use crate::local_pool::refresh::{
    automations::reserve_tests::exhausted_response,
    tests::{cleanup, state},
};

#[tokio::test]
async fn threshold_keeps_quota_and_rule_guards_for_route_degraded_members() {
    use zenith_relay_core::accounts::AccountHealthState;

    let state = state();
    let mut response = exhausted_response(&state);
    let now = current_time_ms();
    let mut records = state.store().unwrap().automations().clone();
    records.tasks[0].weekly_reset_remaining_basis_points = Some(0);
    response.account.account.health = AccountHealthState::Degraded;
    response.account.account.last_error_code = Some("upstream_route_degraded".into());
    assert!(due_window(&response, &records, now).is_some());
    assert!(!response.account.account.is_wake_eligible());
    records.tasks[0].enabled = false;
    assert!(due_window(&response, &records, now).is_none());
    records.tasks[0].enabled = true;
    assert!(
        due_window(
            &response,
            &records,
            now + zenith_relay_core::QUOTA_STALE_AFTER_MS + 1
        )
        .is_none()
    );
    response.account.account.last_error_code = Some("quota_refresh_failed".into());
    assert!(due_window(&response, &records, now).is_none());
    cleanup(state).await;
}

#[tokio::test]
async fn linked_attempt_history_survives_reader_disable_and_selection_changes() {
    use crate::local_pool::models::AccountRouteGroup;
    let state = state();
    let response = exhausted_response(&state);
    let mut reader = response.account;
    reader.account.in_pool = false;
    reader.account.enabled = false;
    let mut selected = reader.clone();
    selected.account.id = "selected-synthetic".into();
    selected.account.enabled = true;
    selected.account.in_pool = true;
    selected.account.quota = Default::default();
    let mut settings = crate::local_pool::models::GatewaySettings {
        account_routes: vec![AccountRouteGroup {
            account_ids: vec![reader.account.id.clone(), selected.account.id.clone()],
            selected_account_id: selected.account.id.clone(),
            notes_account_id: None,
        }],
        ..Default::default()
    };
    let mut records = AutomationRecords::default();
    records
        .weekly_reset_fingerprints
        .insert(reader.account.id.clone(), "synthetic-exhaustion".into());
    assert!(has_pending_confirmation(
        &records,
        &[reader.clone(), selected.clone()],
        &settings,
        &selected.account.id,
    ));
    records
        .weekly_reset_fingerprints
        .insert(reader.account.id.clone(), format!("{PREFIX}200{PENDING}"));
    assert!(has_pending_confirmation(
        &records,
        &[reader.clone(), selected.clone()],
        &settings,
        &selected.account.id
    ));
    assert!(!has_pending_redemption(
        &records,
        &settings,
        "independent-account"
    ));
    assert!(!has_pending_confirmation(
        &records,
        &[reader.clone(), selected.clone()],
        &settings,
        "independent-account"
    ));
    assert!(super::already_attempted(
        &records,
        &settings,
        &selected.account.id,
        300
    ));
    records
        .weekly_reset_fingerprints
        .insert(reader.account.id.clone(), format!("{PREFIX}200"));
    assert!(super::already_attempted(
        &records,
        &settings,
        &selected.account.id,
        100
    ));
    assert!(!super::already_attempted(
        &records,
        &settings,
        &selected.account.id,
        200
    ));
    settings.account_routes[0].selected_account_id = reader.account.id.clone();
    assert!(super::already_attempted(
        &records,
        &settings,
        &selected.account.id,
        100
    ));
    assert!(!super::already_attempted(
        &records,
        &settings,
        "unrelated",
        100
    ));
    cleanup(state).await;
}

#[tokio::test]
async fn threshold_rejects_expired_and_forbidden_subscriptions() {
    use zenith_relay_core::quota::{Subscription, SubscriptionStatus};
    let state = state();
    let mut response = exhausted_response(&state);
    let now = current_time_ms();
    let mut records = state.store().unwrap().automations().clone();
    records.tasks[0].weekly_reset_remaining_basis_points = Some(0);
    for (deadline, status, eligible) in [
        (Some(now + 60_000), SubscriptionStatus::Active, true),
        (None, SubscriptionStatus::Unknown, true),
        (Some(now), SubscriptionStatus::Active, false),
        (Some(now - 1), SubscriptionStatus::Active, false),
        (Some(now + 60_000), SubscriptionStatus::Expired, false),
        (Some(now + 60_000), SubscriptionStatus::Forbidden, false),
    ] {
        response.account.account.subscription = Subscription {
            plan_type: Some("plus".into()),
            active_until_ms: deadline,
            status,
            updated_at_ms: Some(now),
        };
        assert_eq!(due_window(&response, &records, now).is_some(), eligible);
    }
    cleanup(state).await;
}

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
    for minutes in [None, Some(300)] {
        response
            .account
            .account
            .quota
            .secondary
            .as_mut()
            .unwrap()
            .window_minutes = minutes;
        assert!(due_window(&response, &records, now).is_none());
    }
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
    assert!(has_pending_redemption(
        state.store().unwrap().automations(),
        &Default::default(),
        &id,
    ));
    assert!(already_attempted(
        state.store().unwrap().automations(),
        &id,
        deadline + 1
    ));
    for (remaining, minutes) in [(9_950, 10_080), (10_000, 300)] {
        let window = response.account.account.quota.secondary.as_mut().unwrap();
        window.available_basis_points = Some(remaining);
        window.explicitly_full = None;
        window.window_minutes = Some(minutes);
        state
            .store()
            .unwrap()
            .upsert_account(response.account.clone())
            .unwrap();
        confirm_redemption(&state, &response, &fence).unwrap();
        assert!(has_pending_redemption(
            state.store().unwrap().automations(),
            &Default::default(),
            &id,
        ));
    }
    let window = response.account.account.quota.secondary.as_mut().unwrap();
    window.available_basis_points = Some(10_000);
    window.window_minutes = Some(10_080);
    state
        .store()
        .unwrap()
        .upsert_account(response.account.clone())
        .unwrap();
    confirm_redemption(&state, &response, &fence).unwrap();
    assert!(!has_pending_redemption(
        state.store().unwrap().automations(),
        &Default::default(),
        &id,
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

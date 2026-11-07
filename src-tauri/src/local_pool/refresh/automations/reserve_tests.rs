use super::*;
use crate::local_pool::refresh::tests::{account, cleanup, state};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    automations::{AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTask},
    quota::QuotaWindow,
};

fn exhausted_response(state: &DesktopState) -> AccountQuotaRefreshResponse {
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
        assert!(weekly_exhaustion_candidates(&response).is_empty());
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
async fn an_unconfirmed_manual_primary_reset_also_preserves_weekly_reserves() {
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
    assert!(!weekly_reset_still_current(&state, &response, &fence).unwrap());
    other
        .account
        .quota
        .primary
        .as_mut()
        .unwrap()
        .available_basis_points = Some(10_000);
    state.store().unwrap().upsert_account(other).unwrap();
    assert!(weekly_reset_still_current(&state, &response, &fence).unwrap());
    cleanup(state).await;
}

#[tokio::test]
async fn stale_future_or_nonempty_weekly_observations_never_spend_a_reset() {
    let state = state();
    let mut response = exhausted_response(&state);
    let now = current_time_ms();
    assert!(fresh_weekly_exhaustion(&response, now));
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
        assert!(!fresh_weekly_exhaustion(&response, now));
    }
    cleanup(state).await;
}

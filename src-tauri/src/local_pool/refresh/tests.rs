use super::*;
use crate::local_pool::accounts::{credentials::StoredCodexCredentials, records};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use zenith_relay_core::protocol::RefreshStatus;
use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountAuthState, ReauthReason},
    protocol::RemoteAccountLocation,
    quota::{QuotaWindow, QuotaWindowKind},
    scheduler::refresh::{RefreshIdentity, RefreshJobId},
};

pub(super) fn account() -> LocalAccountRecord {
    // Deliberately do not save credentials. No test may contact a provider.
    let credentials = StoredCodexCredentials::new(
        &format!("refresh_test_{}", uuid::Uuid::new_v4().simple()),
        "synthetic-access".into(),
        Some("synthetic-refresh".into()),
        None,
        Some(60_000),
        1,
        1,
        None,
        Some("synthetic-provider".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    records::new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        1,
    )
    .unwrap()
}

pub(super) fn state() -> DesktopState {
    DesktopState::open(
        std::env::temp_dir().join(format!("relay-refresh-host-{}", uuid::Uuid::new_v4())),
    )
    .unwrap()
}

pub(super) async fn cleanup(state: DesktopState) {
    state.refresh.shutdown().await;
    let root = state.root.clone();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn desktop_runtime_inputs_report_saved_account_evidence_without_a_provider_read() {
    let state = state();
    let mut saved = account();
    let id = saved.account.id.clone();
    saved.account.quota.updated_at_ms = Some(1);
    state.store().unwrap().upsert_account(saved).unwrap();
    let inputs = state.runtime_inputs().await.unwrap();
    assert_eq!(inputs.account_refresh[&id].models, RefreshStatus::Stale);
    assert_eq!(inputs.account_refresh[&id].quota, RefreshStatus::Stale);
    cleanup(state).await;
}

#[test]
fn enabled_local_accounts_are_monitored_outside_pool_but_not_after_logout_or_transfer() {
    let mut account = account();
    assert!(!account.account.in_pool);
    account.account.draining = true;
    assert!(automatic_eligible(&account));
    account.account.enabled = false;
    assert!(!automatic_eligible(&account));
    account.account.enabled = true;
    account.account.auth_state = AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant);
    assert!(!automatic_eligible(&account));
    account.account.auth_state = AccountAuthState::DegradedAccessOnly;
    assert!(automatic_eligible(&account));
    account.remote_location = Some(RemoteAccountLocation {
        server_id: "synthetic-server".into(),
        remote_account_id: "synthetic-remote".into(),
    });
    assert!(!automatic_eligible(&account));
}

#[tokio::test]
async fn manual_quota_and_models_share_service_without_fabricating_missing_credential_observations()
{
    let state = state();
    let account = account();
    let id = account.account.id.clone();
    state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();
    let (quota, models) = tokio::join!(
        request(&state, &id, RefreshKind::Quota),
        request(&state, &id, RefreshKind::Models),
    );
    assert_eq!(quota.unwrap_err().code, models.unwrap_err().code);
    assert!(!state.quota_refresh_in_flight(&id).unwrap());
    assert!(!state.refresh_started.load(Ordering::Acquire));
    // Missing secrets are preparation failures, not invented provider quota.
    assert_eq!(state.store().unwrap().account(&id).unwrap(), &account);
    cleanup(state).await;
}

#[tokio::test]
async fn manual_remote_refresh_is_rejected_before_registration_or_credentials() {
    let state = state();
    let mut account = account();
    let id = account.account.id.clone();
    account.remote_location = Some(RemoteAccountLocation {
        server_id: "synthetic-server".into(),
        remote_account_id: "synthetic-remote".into(),
    });
    state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();
    assert_eq!(
        request(&state, &id, RefreshKind::Quota)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(state.store().unwrap().account(&id).unwrap(), &account);
    cleanup(state).await;
}

#[tokio::test]
async fn stale_job_cannot_capture_new_login_or_persist_a_preparation_error() {
    let state = state();
    let account = account();
    let id = account.account.id.clone();
    state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();
    let fence = state.store().unwrap().account_refresh_scope(&id).unwrap().1;
    state
        .store()
        .unwrap()
        .invalidate_account_refresh(&[&id])
        .unwrap();
    for kind in [RefreshKind::Quota, RefreshKind::Models] {
        let job = RefreshJob {
            id: RefreshJobId(1),
            identity: fence.identity(),
            kind,
            due_at_ms: 0,
            manual: true,
        };
        assert_eq!(
            execute(&state, &fence, &job).await.unwrap_err().code,
            ErrorCode::Conflict
        );
    }
    assert_eq!(state.store().unwrap().account(&id).unwrap(), &account);
    cleanup(state).await;
}

#[tokio::test]
async fn desktop_auth_does_not_poll_and_rejects_a_replaced_login_before_secret_read() {
    let state = state();
    let account = account();
    let id = account.account.id.clone();
    state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();
    reconcile(&state).await.unwrap();
    let fence = state.store().unwrap().account_refresh_scope(&id).unwrap().1;
    assert_eq!(
        state
            .refresh
            .freshness(&fence.identity(), RefreshKind::Auth),
        zenith_relay_core::scheduler::refresh::RefreshFreshness::Unknown
    );
    state
        .store()
        .unwrap()
        .invalidate_account_refresh(&[&id])
        .unwrap();
    assert_eq!(
        prepare_authorization(&state, &fence)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(state.store().unwrap().account(&id).unwrap(), &account);
    cleanup(state).await;
}

#[tokio::test]
async fn reset_authorization_keeps_the_caller_fence_and_never_reads_a_replaced_login() {
    let state = state();
    let account = account();
    let id = account.account.id.clone();
    state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();
    let fence = state.store().unwrap().account_refresh_scope(&id).unwrap().1;
    state
        .store()
        .unwrap()
        .invalidate_account_refresh(&[&id])
        .unwrap();
    assert_eq!(
        request_authorization_now(&state, &fence)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        state
            .refresh
            .freshness(&fence.identity(), RefreshKind::Auth),
        zenith_relay_core::scheduler::refresh::RefreshFreshness::Unknown
    );
    assert_eq!(state.store().unwrap().account(&id).unwrap(), &account);
    cleanup(state).await;
}

#[tokio::test]
async fn registrations_do_not_keep_the_desktop_owner_alive() {
    let state = state();
    let account = account();
    state.store().unwrap().upsert_account(account).unwrap();
    reconcile(&state).await.unwrap(); // automatic work is not started
    let weak = Arc::downgrade(&state.owner);
    let root = state.root.clone();
    let service = state.refresh.clone();
    drop(state);
    assert!(weak.upgrade().is_none());
    service.shutdown().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn in_flight_projection_follows_durable_identity_not_retired_jobs() {
    let state = state();
    let account = account();
    let id = account.account.id.clone();
    state.store().unwrap().upsert_account(account).unwrap();
    let fence = state.store().unwrap().account_refresh_scope(&id).unwrap().1;
    let release = Arc::new(Notify::new());
    let unblock = release.clone();
    let (started, mut starts) = mpsc::unbounded_channel();
    state
        .refresh
        .register(
            RefreshRegistration {
                identity: fence.identity(),
                kind: RefreshKind::Quota,
                origin: "https://synthetic.example.test".into(),
                active: false,
                automatic: false,
                due_now: false,
            },
            move |_| {
                let (release, started) = (unblock.clone(), started.clone());
                Box::pin(async move {
                    started.send(()).unwrap();
                    release.notified().await;
                    RefreshResult {
                        value: Err(LocalPoolError::invalid_state("synthetic read")),
                        outcome: RefreshOutcome::Success,
                    }
                })
            },
        )
        .unwrap();
    let service = state.refresh.clone();
    let identity = fence.identity();
    let caller = tokio::spawn(async move { service.request(&identity, RefreshKind::Quota).await });
    starts.recv().await.unwrap();
    assert!(state.quota_refresh_in_flight(&id).unwrap());
    state
        .store()
        .unwrap()
        .invalidate_account_refresh(&[&id])
        .unwrap();
    assert!(!state.quota_refresh_in_flight(&id).unwrap());
    reconcile(&state).await.unwrap();
    assert_eq!(caller.await.unwrap().unwrap_err(), RefreshWaitError::Stale);
    release.notify_one();
    cleanup(state).await;
}

#[tokio::test]
async fn reconciliation_waits_for_setup_transaction_and_uses_latest_persisted_accounts() {
    let state = state();
    let account = account();
    let id = account.account.id.clone();
    state.store().unwrap().upsert_account(account).unwrap();
    let guard = state.setup_guard().await;
    let clone = state.clone();
    let reconcile = tokio::spawn(async move { reconcile(&clone).await });
    state
        .store()
        .unwrap()
        .replace_accounts_and_keys(vec![], vec![])
        .unwrap();
    drop(guard);
    tokio::time::timeout(Duration::from_secs(1), reconcile)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        state
            .refresh
            .request(
                &RefreshIdentity::new(format!("account:{id}"), 1, 0),
                RefreshKind::Quota
            )
            .await
            .unwrap_err(),
        RefreshWaitError::Stale
    );
    cleanup(state).await;
}

#[test]
fn reset_due_uses_reported_future_window_and_bounded_stable_jitter() {
    let mut account = account();
    account.account.quota.primary = Some(QuotaWindow {
        kind: QuotaWindowKind::Primary,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(100),
        explicitly_full: Some(false),
        reset_at_ms: Some(100_000),
        window_minutes: Some(300),
        observed_at_ms: 1,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
    });
    let mut response = AccountQuotaRefreshResponse {
        account,
        quota: AccountQuotaOutcome::Updated {
            transitions: vec![],
            exhaustion_transitions: vec![],
        },
        exhaustion_transitions: vec![],
    };
    let delay = reset_due_delay(&response, 90_000).unwrap();
    assert!((15_000..25_000).contains(&delay));
    assert_eq!(reset_due_delay(&response, 90_000), Some(delay));
    assert_eq!(reset_due_delay(&response, 100_000), None);
    response.quota = AccountQuotaOutcome::Skipped;
    assert_eq!(reset_due_delay(&response, 90_000), None);
}

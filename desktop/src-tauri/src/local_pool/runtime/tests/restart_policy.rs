use super::*;
use std::sync::Arc;
use zenith_relay_core::accounts::{AccountAuthMode, AccountHealthState};
use zenith_relay_core::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind};

fn assert_restart_policy(
    runtime: &GatewayRuntime,
    exhausted_id: &str,
    healthy_id: &str,
    reset_at: u64,
) {
    let candidates = runtime.candidate_runtime_order();
    let exhausted = candidates
        .iter()
        .find(|candidate| candidate.candidate_id == exhausted_id)
        .unwrap();
    assert!(!exhausted.available);
    assert!(exhausted.quota_wait);
    assert_eq!(exhausted.next_retry_at_ms, Some(reset_at));
    let healthy = candidates
        .iter()
        .find(|candidate| candidate.candidate_id == healthy_id)
        .unwrap();
    assert!(
        healthy.available,
        "legacy health hints must not create a hard restart block"
    );
    assert_eq!(healthy.next_retry_at_ms, None);
    assert!(!healthy.half_open);
    assert_eq!(healthy.in_flight, 0);
}

#[tokio::test]
async fn restart_keeps_provider_quota_deadline_separate_from_legacy_health_hints() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = crate::files::test_temp_root("runtime-restart-policy");
    let now = current_time_ms();
    let reset_at = now + 3_600_000;
    let exhausted_id = format!("synthetic-exhausted-{id}");
    let healthy_id = format!("synthetic-healthy-{id}");
    let credentials_store = CredentialStore::from_backend(NativeSecretBackend);
    let state = DesktopState::open(root.clone()).unwrap();
    for (account_id, exhausted) in [(&exhausted_id, true), (&healthy_id, false)] {
        let credentials = StoredCodexCredentials::new(
            account_id,
            "synthetic-access".into(),
            Some("synthetic-refresh".into()),
            None,
            Some(now + 60_000),
            now,
            1,
            None,
            Some(format!("synthetic-provider-{account_id}")),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .unwrap()
        .with_oauth_client_kind(zenith_relay_core::providers::chatgpt::OAuthClientKind::ExcelBps);
        credentials_store.save(&credentials).unwrap();
        let mut account = records::new_account_record(
            &credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            now,
        )
        .unwrap();
        account.account.in_pool = true;
        account.account.health = AccountHealthState::Degraded;
        account.account.quota = QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                provider_cycle_id: None,
                window_start_ms: Some(now),
                available_basis_points: Some(if exhausted { 0 } else { 5_000 }),
                explicitly_full: Some(false),
                reset_at_ms: Some(reset_at),
                window_minutes: Some(300),
                observed_at_ms: now,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            }),
            limit_reached: exhausted,
            updated_at_ms: Some(now),
            ..Default::default()
        };
        account.cooldowns.insert("*".into(), reset_at + 3_600_000);
        account.consecutive_failures = 7;
        state.store().unwrap().upsert_account(account).unwrap();
    }
    let runtime = runtime_from_store(&state).await.unwrap();
    assert_restart_policy(&runtime, &exhausted_id, &healthy_id, reset_at);
    let retired = Arc::downgrade(&runtime);
    let database = Arc::downgrade(&state.telemetry);
    drop(runtime);
    drop(state);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while retired.strong_count() != 0 || database.strong_count() != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the first fixture must release runtime and storage before restart");

    let state = DesktopState::open(root.clone()).unwrap();
    {
        let store = state.store().unwrap();
        let saved = store.account(&exhausted_id).unwrap();
        assert_eq!(
            saved.account.quota.primary.as_ref().unwrap().reset_at_ms,
            Some(reset_at)
        );
        assert!(saved.account.quota.limit_reached);
    }
    let runtime = runtime_from_store(&state).await.unwrap();
    assert!(!retired.ptr_eq(&Arc::downgrade(&runtime)));
    assert_restart_policy(&runtime, &exhausted_id, &healthy_id, reset_at);
    for key in state.store().unwrap().keys() {
        secret_store::delete(&key.secret_ref).unwrap();
    }
    credentials_store.delete(&exhausted_id).unwrap();
    credentials_store.delete(&healthy_id).unwrap();
    let database = Arc::downgrade(&state.telemetry);
    drop(runtime);
    drop(state);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while database.strong_count() != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture storage must close before cleanup");
    fs::remove_dir_all(root).unwrap();
}

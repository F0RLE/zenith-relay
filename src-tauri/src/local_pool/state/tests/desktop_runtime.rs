use super::*;

#[test]
fn quota_refreshes_share_one_lock_per_account() {
    let root = temp_root("quota-locks");
    let state = DesktopState::open(root.clone()).unwrap();
    let first = state.quota_account_lock("account-1").unwrap();
    let same = state.quota_account_lock("account-1").unwrap();
    let other = state.quota_account_lock("account-2").unwrap();

    assert!(Arc::ptr_eq(&first, &same));
    assert!(!Arc::ptr_eq(&first, &other));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn background_workers_stay_active_for_the_desktop_process_lifetime() {
    let root = temp_root("background-session");
    let state = DesktopState::open(root.clone()).unwrap();

    assert!(state.background_session_active());
    state.wait_for_background_session_active().await;
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(10),
        state.wait_for_background_session_inactive()
    )
    .await
    .is_err());

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn catalog_refresh_warning_survives_restart_and_clears_after_success() {
    let root = temp_root("catalog-refresh-warning");
    let state = DesktopState::open(root.clone()).unwrap();
    let error = LocalPoolError::new(ErrorCode::GatewayUnavailable, "provider unavailable");
    state.record_catalog_refresh_result(Some(&error));
    assert_eq!(
        state.catalog_refresh_warning().as_deref(),
        Some("model_catalog_refresh_failed:gateway_unavailable")
    );
    drop(state);

    let state = DesktopState::open(root.clone()).unwrap();
    assert_eq!(
        state.catalog_refresh_warning().as_deref(),
        Some("model_catalog_refresh_failed:gateway_unavailable")
    );
    state.record_catalog_refresh_result(None);
    drop(state);

    let state = DesktopState::open(root.clone()).unwrap();
    assert!(state.catalog_refresh_warning().is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn deferred_catalog_refresh_warning_survives_restart_without_an_error_timestamp() {
    let root = temp_root("catalog-refresh-deferred");
    let state = DesktopState::open(root.clone()).unwrap();
    state.record_catalog_refresh_deferred();
    assert_eq!(
        state.catalog_refresh_warning().as_deref(),
        Some("model_catalog_refresh_deferred:codex_running")
    );
    assert!(state
        .store()
        .unwrap()
        .gateway()
        .catalog_refresh_error_at_ms
        .is_none());
    drop(state);

    let state = DesktopState::open(root.clone()).unwrap();
    assert_eq!(
        state.catalog_refresh_warning().as_deref(),
        Some("model_catalog_refresh_deferred:codex_running")
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn subscription_refreshes_share_one_global_lock() {
    let root = temp_root("subscription-lock");
    let state = DesktopState::open(root.clone()).unwrap();
    let first = state.subscription_refresh_guard().await;

    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(10),
        state.subscription_refresh_guard(),
    )
    .await
    .is_err());
    drop(first);
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(10),
        state.subscription_refresh_guard(),
    )
    .await
    .is_ok());

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn opening_storage_preserves_accounts_without_starting_provider_work() {
    let root = temp_root("refresh-startup");
    {
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let mut outside_pool = account_record("account-3");
        outside_pool.account.in_pool = false;
        store
            .replace_accounts_and_keys(
                vec![
                    account_record("account-1"),
                    account_record("account-2"),
                    outside_pool,
                ],
                Vec::new(),
            )
            .unwrap();
    }
    let state = DesktopState::open(root.clone()).unwrap();
    assert!(!state.refresh_started.load(Ordering::Acquire));
    assert_eq!(state.store().unwrap().accounts().len(), 3);
    for id in ["account-1", "account-2", "account-3"] {
        assert!(!state.quota_refresh_in_flight(id).unwrap());
        assert!(state.sync_account_quota_refresh(id, now_ms()).unwrap());
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn setup_guard_serializes_mutations() {
    let root = std::env::temp_dir().join(format!("zenith-relay-lock-{}", uuid::Uuid::new_v4()));
    let state = Arc::new(DesktopState::open(root.clone()).unwrap());
    let first = state.setup_guard().await;
    let waiting_state = state.clone();
    let waiting = tokio::spawn(async move {
        let _guard = waiting_state.setup_guard().await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!waiting.is_finished());
    drop(first);
    waiting.await.unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

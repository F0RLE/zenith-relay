use super::*;
use std::sync::Arc;
use zenith_relay_core::{GatewayRuntime, LocalGatewayKey, ProviderSource, WireApi};

#[tokio::test]
async fn incomplete_delete_rollback_closes_the_old_gateway() {
    let root = std::env::temp_dir().join(format!(
        "relay-delete-fail-closed-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let runtime = Arc::new(
        GatewayRuntime::new(
            ProviderSource {
                id: "synthetic-source".into(),
                name: "Synthetic".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                api_key: "synthetic-upstream".into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
            },
            LocalGatewayKey {
                id: "synthetic-key".into(),
                secret: "synthetic-local".into(),
            },
            Arc::new(|_| {}),
        )
        .unwrap(),
    );
    state.gateway.start(runtime.clone(), 0).await.unwrap();
    state.store().unwrap().set_gateway_enabled(true).unwrap();
    let _dispatch_fences =
        fence_runtime_candidates(Some(&runtime), &[], &["synthetic-source".into()]);
    assert!(!_dispatch_fences.is_empty());

    let deleted = PreparedAccountDelete {
        old_credential: None,
        previous_wake: state.wake_snapshot().unwrap(),
        old_automations: state.store().unwrap().automations().clone(),
        restored_bindings: vec![codex::ProfileBinding {
            profile_dir: root.join("profile").to_string_lossy().into_owned(),
            credential_kind: codex::ProfileCredentialKind::OAuthAccount,
            credential_id: "synthetic-account".into(),
            bound_oauth_account_id: None,
            active: true,
        }],
        previous_proxy_pool: None,
    };
    let cause = LocalPoolError::new(ErrorCode::Io, "injected deletion failure");
    let rollback = rollback_prepared_delete(
        &state,
        &CredentialStore::from_backend(NativeSecretBackend),
        &deleted,
        &cause,
    );
    assert_eq!(
        rollback.as_ref().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert!(state.gateway.runtime().await.is_some());
    let error = ensure_delete_rollback_or_fail_closed(&state, rollback)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(state.gateway.runtime().await.is_none());
    assert!(!state.store().unwrap().gateway().enabled);

    drop(_dispatch_fences);
    drop(runtime);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn delete_waits_for_the_same_credential_lock_as_refresh() {
    use futures_util::poll;
    use std::task::Poll;

    let root = std::env::temp_dir().join(format!(
        "relay-delete-credential-lock-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .unwrap();
    let held = locks.acquire("synthetic_account").await.unwrap();
    let mut deleting = Box::pin(acquire_delete_credential_guards(
        &state,
        &["synthetic_account"],
    ));
    assert!(matches!(poll!(deleting.as_mut()), Poll::Pending));
    drop(held);
    let guards = tokio::time::timeout(std::time::Duration::from_secs(2), deleting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(guards.len(), 1);
    drop(guards);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

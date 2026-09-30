use super::*;
use crate::local_pool::accounts::{credentials::StoredCodexCredentials, records};
use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountAuthState, AccountHealthState},
    providers::chatgpt::ModelDiscoveryFailureCode,
    quota::{
        QuotaRefreshData, QuotaRefreshResult, QuotaWindowInput, QuotaWindowKind, SubscriptionInput,
    },
};

fn account() -> LocalAccountRecord {
    let credentials = StoredCodexCredentials::new(
        "test-account",
        "test-access".into(),
        Some("test-refresh".into()),
        None,
        Some(60_000),
        1,
        1,
        None,
        Some("test-provider-account".into()),
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

fn with_store(test: impl FnOnce(&mut LocalPoolStore, &AccountRefreshScope)) {
    let root = std::env::temp_dir().join(format!(
        "relay-refresh-observation-{}",
        uuid::Uuid::new_v4()
    ));
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store.upsert_account(account()).unwrap();
    let (before, fence) = store.account_refresh_scope("test-account").unwrap();
    let scope = AccountRefreshScope {
        before,
        fence,
        started_at_ms: 100,
    };
    test(&mut store, &scope);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

fn quota(percent: f64, observed_at_ms: u64) -> QuotaRefreshOutcome {
    QuotaRefreshOutcome::Updated(Box::new(QuotaRefreshResult {
        quota: QuotaRefreshData {
            primary: Some(QuotaWindowInput {
                kind: QuotaWindowKind::Primary,
                available_percent: Some(percent),
                explicitly_full: None,
                reset: None,
                window_minutes: Some(300),
                provider_cycle_id: None,
                observed_at_ms,
            }),
            subscription: Some(SubscriptionInput {
                plan_type: Some("plus".into()),
                active_until_ms: None,
                forbidden: false,
                observed_at_ms,
            }),
            observed_at_ms,
            ..Default::default()
        },
        allowed: Some(true),
        reported_limit_reached: Some(false),
    }))
}

fn quota_failure() -> QuotaRefreshOutcome {
    QuotaRefreshOutcome::Failed {
        failure: QuotaRefreshFailure::new("quota_transport", true),
        subscription: Subscription::default(),
    }
}

fn models_failure() -> std::result::Result<Vec<String>, ModelDiscoveryFailure> {
    Err(ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::Transport,
        retryable: true,
        http_status: None,
        retry_after_ms: None,
    })
}

#[test]
fn late_quota_success_failure_and_preparation_error_preserve_newer_passive_quota() {
    for observed_at in [50, 100, 200] {
        with_store(|store, scope| {
            let mut current = scope.before.clone();
            let QuotaRefreshOutcome::Updated(data) = quota(45.0, observed_at) else {
                unreachable!()
            };
            super::super::quota_service::apply_quota_success(&mut current, *data).unwrap();
            store.upsert_account(current.clone()).unwrap();
            for late in [quota(0.0, 100), quota(100.0, 100), quota_failure()] {
                let applied =
                    apply_quota_read(store, scope, late, Subscription::default(), None).unwrap();
                assert!(matches!(
                    applied.value.outcome,
                    AccountQuotaOutcome::Skipped
                ));
                assert!(applied.value.exhaustion_transitions.is_empty());
                assert_eq!(applied.account, current);
            }
            apply_read_error(
                store,
                scope,
                RefreshReadKind::Quota,
                &LocalPoolError::new(ErrorCode::GatewayUnavailable, "synthetic route failure"),
            )
            .unwrap();
            assert_eq!(store.account("test-account"), Some(&current));
        });
    }
}

#[test]
fn newer_subscription_and_health_win_while_quota_and_inventory_can_update() {
    with_store(|store, scope| {
        let mut current = scope.before.clone();
        current.account.subscription.plan_type = Some("pro".into());
        current.account.subscription.updated_at_ms = Some(200);
        current.account.health = AccountHealthState::Blocked;
        current.account.last_error_code = Some("deactivated_workspace".into());
        current.account.auth_state = AccountAuthState::Error;
        current.weight = 9;
        current.allowed_models = vec!["gpt-new".into()];
        store.upsert_account(current.clone()).unwrap();
        let applied = apply_quota_read(
            store,
            scope,
            quota(80.0, 100),
            Subscription::default(),
            Some(Ok(vec!["gpt-new".into()])),
        )
        .unwrap();
        assert!(matches!(
            applied.value.outcome,
            AccountQuotaOutcome::Updated { .. }
        ));
        assert!(applied.value.models_changed);
        assert_eq!(
            applied.account.account.subscription,
            current.account.subscription
        );
        assert_eq!(applied.account.account.health, current.account.health);
        assert_eq!(
            applied.account.account.auth_state,
            current.account.auth_state
        );
        assert_eq!(
            applied.account.account.last_error_code,
            current.account.last_error_code
        );
        assert_eq!(applied.account.allowed_models, current.allowed_models);
        assert_eq!(applied.account.weight, 9);
        assert_eq!(applied.account.effective_models(), ["gpt-new"]);
    });
}

#[test]
fn models_and_quota_modify_only_their_own_observations() {
    with_store(|store, scope| {
        let models = apply_models_read(store, scope, Ok(vec!["gpt-new".into()])).unwrap();
        assert!(models.value);
        assert_eq!(models.account.account.quota, scope.before.account.quota);
        let quota = apply_quota_read(
            store,
            scope,
            quota(70.0, 100),
            scope.before.account.subscription.clone(),
            None,
        )
        .unwrap();
        assert_eq!(quota.account.effective_models(), ["gpt-new"]);
        let failed = apply_models_read(store, scope, models_failure()).unwrap();
        assert!(!failed.value);
        assert_eq!(failed.account.effective_models(), ["gpt-new"]);
        assert_eq!(failed.account.account.quota, quota.account.account.quota);
    });
}

#[test]
fn rejected_revision_never_applies_success_or_failure_for_either_kind() {
    with_store(|store, scope| {
        store.invalidate_account_refresh(&["test-account"]).unwrap();
        let current = store.account("test-account").unwrap().clone();
        assert!(apply_models_read(store, scope, Ok(vec!["gpt-stale".into()])).is_err());
        assert!(apply_models_read(store, scope, models_failure()).is_err());
        assert!(
            apply_quota_read(store, scope, quota(0.0, 100), Subscription::default(), None).is_err()
        );
        assert!(
            apply_quota_read(store, scope, quota_failure(), Subscription::default(), None).is_err()
        );
        for kind in [RefreshReadKind::Models, RefreshReadKind::Quota] {
            assert!(apply_read_error(
                store,
                scope,
                kind,
                &LocalPoolError::new(ErrorCode::Io, "synthetic storage failure")
            )
            .is_err());
        }
        assert_eq!(store.account("test-account"), Some(&current));
    });
}

#[test]
fn model_failure_does_not_overwrite_a_newer_authentication_failure() {
    with_store(|store, scope| {
        let mut current = scope.before.clone();
        current.account.health = AccountHealthState::Unhealthy;
        current.account.last_error_code = Some("token_invalidated".into());
        store.upsert_account(current.clone()).unwrap();
        let applied = apply_models_read(store, scope, models_failure()).unwrap();
        assert_eq!(applied.account.account.health, current.account.health);
        assert_eq!(
            applied.account.account.last_error_code,
            current.account.last_error_code
        );
        apply_read_error(
            store,
            scope,
            RefreshReadKind::Models,
            &LocalPoolError::new(
                ErrorCode::SecretStoreUnavailable,
                "synthetic secret-store failure",
            ),
        )
        .unwrap();
        assert_eq!(store.account("test-account"), Some(&current));
    });
}

#[test]
fn superseded_quota_does_not_prevent_an_independent_model_result() {
    with_store(|store, scope| {
        let mut current = scope.before.clone();
        current.account.quota.updated_at_ms = Some(200);
        store.upsert_account(current.clone()).unwrap();
        let applied = apply_quota_read(
            store,
            scope,
            quota(0.0, 100),
            Subscription::default(),
            Some(Ok(vec!["gpt-new".into()])),
        )
        .unwrap();
        assert!(matches!(
            applied.value.outcome,
            AccountQuotaOutcome::Skipped
        ));
        assert!(applied.value.models_changed);
        assert_eq!(applied.account.account.quota, current.account.quota);
        assert_eq!(applied.account.effective_models(), ["gpt-new"]);
    });
}

#[tokio::test]
async fn scope_capture_waits_for_setup_transaction_and_late_errors_are_discarded() {
    let root = std::env::temp_dir().join(format!("relay-refresh-scope-{}", uuid::Uuid::new_v4()));
    let state = DesktopState::open(root.clone()).unwrap();
    state.store().unwrap().upsert_account(account()).unwrap();
    let scope = AccountRefreshScope::capture(&state, "test-account")
        .await
        .unwrap();
    let current = {
        let guard = state.setup_guard().await;
        let pending = AccountRefreshScope::capture(&state, "test-account");
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut pending)
                .await
                .is_err()
        );
        state
            .store()
            .unwrap()
            .invalidate_account_refresh(&["test-account"])
            .unwrap();
        drop(guard);
        pending.await.unwrap()
    };
    assert_ne!(scope.fence, current.fence);
    for kind in [RefreshReadKind::Models, RefreshReadKind::Quota] {
        record_read_error(
            &state,
            &scope,
            kind,
            &LocalPoolError::new(ErrorCode::Io, "synthetic storage failure"),
        )
        .await;
    }
    assert_eq!(
        state.store().unwrap().account("test-account"),
        Some(&current.before)
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

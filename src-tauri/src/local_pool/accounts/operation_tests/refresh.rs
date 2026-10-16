use super::*;

#[tokio::test]
async fn bulk_quota_refresh_keeps_a_small_future_and_reports_every_failed_account() {
    let root = std::env::temp_dir().join(format!("relay-quota-batch-{}", Uuid::new_v4()));
    let state = DesktopState::open(root.clone()).unwrap();
    let ids = (0..QUOTA_REFRESH_BATCH_SIZE + 2)
        .map(|_| format!("missing_{}", Uuid::new_v4().simple()))
        .collect::<Vec<_>>();
    let refresh = refresh_account_quotas(&state, ids.clone());

    // Tauri constructs command futures on the Windows UI thread (1 MiB).
    // Embedding five complete refreshes in this future multiplies its stack
    // footprint again in the generated IPC dispatcher, even for other commands.
    assert!(
        std::mem::size_of_val(&refresh) < 16 * 1024,
        "bulk quota refresh must keep concurrent account futures off the IPC stack"
    );
    let results = refresh.await;
    assert_eq!(results.len(), ids.len());
    for (result, id) in results.iter().zip(&ids) {
        assert_eq!(&result.account_id, id);
        assert_eq!(result.status, AccountQuotaRefreshStatus::Failed);
        assert!(result.response.is_none());
        assert!(result.error.is_some());
        assert!(!state.quota_refresh_in_flight(id).unwrap());
    }
    assert!(refresh_account_quotas(&state, Vec::new()).await.is_empty());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn imported_model_lists_are_trimmed_deduplicated_and_validated() {
    assert_eq!(
        normalize_models(vec![" gpt-test ".into(), "GPT-test".into(), "   ".into(),]).unwrap(),
        vec!["gpt-test"]
    );
    assert!(normalize_models(vec!["model\nid".into()]).is_err());
}
#[test]
fn quota_refresh_preserves_a_failure_observed_while_it_was_in_flight() {
    let before_refresh = account_record("account-race");
    let mut latest_account = before_refresh.clone();
    latest_account.account.health = AccountHealthState::Degraded;
    latest_account.account.last_error_code = Some("upstream_rate_limited".into());
    latest_account.cooldowns.insert("*".into(), 500);
    latest_account.consecutive_failures = 2;
    let mut refreshed = before_refresh.clone();

    preserve_newer_account_state(&mut refreshed, &before_refresh, &latest_account);

    assert!(refreshed.cooldowns.is_empty());
    assert_eq!(refreshed.consecutive_failures, 0);
    assert_eq!(refreshed.account.health, AccountHealthState::Degraded);
    assert_eq!(
        refreshed.account.last_error_code.as_deref(),
        Some("upstream_rate_limited")
    );
}
#[test]
fn quota_refresh_merges_auth_and_probe_state_independently() {
    let before_refresh = account_record("account-auth-race");
    let mut latest_account = before_refresh.clone();
    latest_account.account.auth_state = AccountAuthState::RequiresReauth(
        zenith_relay_core::accounts::ReauthReason::ReusedRefreshToken,
    );
    let mut refreshed = before_refresh.clone();
    refreshed.account.health = AccountHealthState::Unhealthy;
    refreshed.account.last_error_code = Some("token_invalidated".into());

    preserve_newer_account_state(&mut refreshed, &before_refresh, &latest_account);

    assert!(matches!(
        refreshed.account.auth_state,
        AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::ReusedRefreshToken
        )
    ));
    assert_eq!(refreshed.account.health, AccountHealthState::Unhealthy);
    assert_eq!(
        refreshed.account.last_error_code.as_deref(),
        Some("token_invalidated")
    );
}
#[test]
fn quota_recovery_uses_http_401_instead_of_a_fixed_error_list() {
    for body in [
        br#"{"detail":{"code":"token_invalidated"}}"#.as_slice(),
        br#"{"detail":{"code":"future_auth_error"}}"#.as_slice(),
        b"".as_slice(),
    ] {
        let result = Ok(QuotaRefreshOutcome::Failed {
            failure: zenith_relay_core::quota::classify_quota_http_failure(401, body),
            subscription: Subscription::default(),
        });
        assert!(quota_refresh_was_unauthorized(&result));
    }

    let payment = Ok(QuotaRefreshOutcome::Failed {
        failure: zenith_relay_core::quota::classify_quota_http_failure(
            402,
            br#"{"detail":{"code":"future_billing_error"}}"#,
        ),
        subscription: Subscription::default(),
    });
    assert!(!quota_refresh_was_unauthorized(&payment));
}
#[test]
fn failed_quota_refresh_still_applies_fetched_subscription() {
    let mut account = account_record("account-subscription-on-failure");
    let subscription = Subscription::normalize(zenith_relay_core::quota::SubscriptionInput {
        plan_type: Some("plus".into()),
        active_until_ms: Some(1_787_544_851_000),
        forbidden: false,
        observed_at_ms: 123,
    });

    let outcome = apply_quota_outcome(
        &mut account,
        QuotaRefreshOutcome::Failed {
            failure: QuotaRefreshFailure::new("quota_transport", true),
            subscription,
        },
        124,
    );

    assert!(matches!(outcome, AccountQuotaOutcome::Failed { .. }));
    assert_eq!(
        account.account.subscription.active_until_ms,
        Some(1_787_544_851_000)
    );
}
#[test]
fn model_refresh_accepts_unknown_slugs_and_preserves_last_good_list() {
    let mut account = account_record("account_models");
    assert!(apply_model_discovery(
        &mut account,
        Ok(vec!["gpt-future-codex".into()])
    ));
    assert_eq!(account.models, ["gpt-test"]);
    assert!(account
        .discovered_models
        .as_ref()
        .is_some_and(|models| models.len() == 1 && models[0] == "gpt-future-codex"));
    assert_eq!(account.effective_models(), ["gpt-future-codex"]);

    assert!(!apply_model_discovery(
        &mut account,
        Err(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::Transport,
            retryable: true,
            retry_after_ms: None,
            http_status: None,
        }),
    ));
    assert_eq!(account.models, ["gpt-test"]);
    assert_eq!(account.effective_models(), ["gpt-future-codex"]);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("models_transport")
    );

    account.models.clear();
    account.discovered_models = None;
    assert!(!apply_model_discovery(
        &mut account,
        Err(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::Transport,
            retryable: true,
            retry_after_ms: None,
            http_status: None,
        }),
    ));
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("models_transport")
    );
    assert_eq!(account.account.health, AccountHealthState::Degraded);

    assert!(apply_model_discovery(
        &mut account,
        Ok(vec!["gpt-recovered".into()])
    ));
    assert_eq!(account.models, ["gpt-recovered"]);
    assert!(account
        .discovered_models
        .as_ref()
        .is_some_and(|models| models.len() == 1 && models[0] == "gpt-recovered"));
    assert_eq!(account.effective_models(), ["gpt-recovered"]);
    assert_eq!(account.account.health, AccountHealthState::Healthy);
    assert!(account.account.last_error_code.is_none());
}
#[test]
fn empty_model_refresh_does_not_replace_a_live_list() {
    let mut account = account_record("account_empty_models");
    let before = account.effective_models().to_vec();
    account.account.health = AccountHealthState::Degraded;
    account.account.last_error_code = Some("models_transport".into());

    assert!(!apply_model_discovery(&mut account, Ok(Vec::new())));

    assert_eq!(account.account.health, AccountHealthState::Degraded);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("models_transport")
    );

    assert!(account.discovered_models.is_none());
    assert_eq!(account.effective_models(), before);
}
#[test]
fn successful_model_refresh_recovers_a_transient_auth_error() {
    let mut account = account_record("account_models_recovered_auth");
    account.account.auth_state = AccountAuthState::Error;
    account.account.health = AccountHealthState::Unhealthy;
    account.account.last_error_code = Some("models_unauthorized".into());

    assert!(apply_model_discovery(
        &mut account,
        Ok(vec!["gpt-recovered".into()])
    ));

    assert_eq!(account.account.auth_state, AccountAuthState::Active);
    assert_eq!(account.account.health, AccountHealthState::Healthy);
    assert_eq!(account.account.last_error_code, None);
}
#[test]
fn model_refresh_detects_provider_order_changes() {
    let mut account = account_record("account_model_order");
    assert!(apply_model_discovery(
        &mut account,
        Ok(vec!["gpt-first".into(), "gpt-second".into()])
    ));
    assert_eq!(account.effective_models(), ["gpt-first", "gpt-second"]);

    assert!(apply_model_discovery(
        &mut account,
        Ok(vec!["gpt-second".into(), "gpt-first".into()])
    ));
    assert_eq!(account.effective_models(), ["gpt-second", "gpt-first"]);
}
#[test]
fn model_unauthorized_removes_an_account_with_cached_models_from_routing() {
    let mut account = account_record("account_models_unauthorized");
    let failure = ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::Unauthorized,
        retryable: false,
        retry_after_ms: None,
        http_status: Some(401),
    };
    assert!(model_discovery_was_unauthorized(&Some(
        Err(failure.clone())
    )));

    assert!(!apply_model_discovery(&mut account, Err(failure)));

    assert!(!account.models.is_empty());
    assert_eq!(account.account.auth_state, AccountAuthState::Error);
    assert_eq!(account.account.health, AccountHealthState::Unhealthy);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("models_unauthorized")
    );
}
#[test]
fn model_unauthorized_does_not_downgrade_reauthentication() {
    let mut account = account_record("account_models_reauth");
    account.account.auth_state = AccountAuthState::RequiresReauth(
        zenith_relay_core::accounts::ReauthReason::AccessTokenExpired,
    );
    account.account.last_error_code = Some("auth_access_token_expired".into());
    let failure = ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::Unauthorized,
        retryable: false,
        retry_after_ms: None,
        http_status: Some(401),
    };

    assert!(!apply_model_discovery(&mut account, Err(failure)));
    assert_eq!(
        account.account.auth_state,
        AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::AccessTokenExpired,
        )
    );
    assert_eq!(account.account.health, AccountHealthState::Unhealthy);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("auth_access_token_expired")
    );
}
#[test]
fn failed_account_without_models_remains_manageable() {
    let mut account = account_record("account_failed");
    account.models.clear();
    account.account.health = zenith_relay_core::accounts::AccountHealthState::Unhealthy;
    account.account.last_error_code = Some("models_unauthorized".into());
    assert!(account_model_state_is_valid(&account));

    account.account.health = zenith_relay_core::accounts::AccountHealthState::Healthy;
    assert!(!account_model_state_is_valid(&account));
}
#[test]
fn successful_empty_account_catalog_remains_manageable() {
    let mut account = account_record("account_empty_catalog");
    account.models.clear();
    account.discovered_models = Some(Vec::new());
    account.account.health = AccountHealthState::Healthy;
    account.account.last_error_code = None;

    assert!(account_model_state_is_valid(&account));
}
#[test]
fn quota_response_types_are_safe_and_serializable() {
    let response = AccountQuotaOutcome::Failed {
        code: "quota_transport".into(),
        retryable: true,
    };
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(serialized.contains("quota_transport"));
    assert!(!serialized.contains("Bearer"));
    let _ = WireApi::Responses;
}

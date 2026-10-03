use super::*;

#[test]
fn stable_identity_uses_only_hashed_inputs() {
    let first = AccountIdentity::from_hashed_parts(
        "openai",
        "api.example.test/v1",
        "email-hash",
        "secret-hash",
        "default",
        Some("org-hash"),
    )
    .unwrap();
    let second = AccountIdentity::from_hashed_parts(
        "OPENAI",
        "API.EXAMPLE.TEST/V1",
        "EMAIL-HASH",
        "SECRET-HASH",
        "DEFAULT",
        Some("ORG-HASH"),
    )
    .unwrap();

    assert_eq!(first, second);
    assert!(!format!("{first:?}").contains('@'));
    assert_eq!(first.stable_index.len(), 64);
}

#[test]
fn exact_provider_failures_have_one_terminal_account_state() {
    assert_eq!(
        provider_account_failure("token_invalidated"),
        Some(ProviderAccountFailure::Authentication)
    );
    assert_eq!(
        provider_account_failure("deactivated_workspace"),
        Some(ProviderAccountFailure::Blocked)
    );
    assert_eq!(provider_account_failure("quota_unauthorized"), None);
}

#[test]
fn automatic_quota_monitoring_is_independent_of_pool_and_health() {
    assert!(automatic_quota_monitoring_eligible(
        true,
        AccountAuthState::Active,
    ));
    assert!(automatic_quota_monitoring_eligible(
        true,
        AccountAuthState::DegradedAccessOnly,
    ));
    assert!(automatic_quota_monitoring_eligible(
        true,
        AccountAuthState::Error,
    ));
    assert!(!automatic_quota_monitoring_eligible(
        false,
        AccountAuthState::Active,
    ));
    assert!(!automatic_quota_monitoring_eligible(
        true,
        AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
    ));
    assert!(automatic_quota_monitoring_eligible(
        true,
        AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken),
    ));
}

#[test]
fn refresh_token_reused_is_not_a_provider_authentication_failure() {
    assert_eq!(provider_account_failure("refresh_token_reused"), None);
    assert_eq!(
        provider_account_failure("refresh_token_expired"),
        Some(ProviderAccountFailure::Authentication)
    );
}

#[test]
fn model_discovery_state_transitions_keep_user_reauthentication_intact() {
    let mut auth_state = AccountAuthState::Active;
    let mut health = AccountHealthState::Healthy;
    let mut last_error_code = None;

    apply_model_discovery_failure(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
        "models_transport",
        true,
    );
    assert_eq!(auth_state, AccountAuthState::Active);
    assert_eq!(health, AccountHealthState::Degraded);
    assert_eq!(last_error_code.as_deref(), Some("models_transport"));
    assert!(recover_model_discovery_state(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
    ));
    assert_eq!(health, AccountHealthState::Healthy);
    assert!(last_error_code.is_none());

    auth_state = AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant);
    health = AccountHealthState::Unhealthy;
    last_error_code = Some("invalid_grant".into());
    apply_model_discovery_failure(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
        "models_unauthorized",
        false,
    );
    assert!(auth_state.requires_fresh_login());
    assert_eq!(health, AccountHealthState::Unhealthy);
    assert!(!recover_model_discovery_state(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
    ));
    assert_eq!(health, AccountHealthState::Unhealthy);
    assert_eq!(last_error_code.as_deref(), Some("invalid_grant"));

    auth_state = AccountAuthState::Active;
    health = AccountHealthState::Healthy;
    last_error_code = None;
    apply_model_discovery_failure(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
        "models_forbidden",
        false,
    );
    assert_eq!(health, AccountHealthState::Degraded);

    // A previously confirmed account block must not be cleared by a
    // separate model-discovery permission failure.
    health = AccountHealthState::Blocked;
    apply_model_discovery_failure(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
        "models_forbidden",
        false,
    );
    assert_eq!(health, AccountHealthState::Blocked);
}

fn usage_state() -> AccountUsageState {
    AccountUsageState {
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        last_error_code: None,
        last_used_at_ms: None,
    }
}

#[test]
fn model_discovery_preserves_independent_account_failures() {
    use crate::{account_candidate_health, quota::SubscriptionStatus};

    for (initial_auth, initial_health, initial_error) in [
        (
            AccountAuthState::Active,
            AccountHealthState::Blocked,
            "workspace_disabled",
        ),
        (
            AccountAuthState::Active,
            AccountHealthState::Unhealthy,
            "token_invalidated",
        ),
        (
            AccountAuthState::Error,
            AccountHealthState::Unhealthy,
            "upstream_unauthorized",
        ),
        (
            AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
            AccountHealthState::Unhealthy,
            "invalid_grant",
        ),
        (
            AccountAuthState::Active,
            AccountHealthState::Degraded,
            "checkpoint",
        ),
        (
            AccountAuthState::Active,
            AccountHealthState::Degraded,
            "captcha",
        ),
        (
            AccountAuthState::Active,
            AccountHealthState::Degraded,
            "upstream_account_verification_required",
        ),
    ] {
        for (code, retryable) in [
            ("models_transport", true),
            ("models_forbidden", false),
            ("models_unauthorized", false),
        ] {
            let mut auth_state = initial_auth;
            let mut health = initial_health;
            let mut last_error_code = Some(initial_error.to_string());
            apply_model_discovery_failure(
                &mut auth_state,
                &mut health,
                &mut last_error_code,
                code,
                retryable,
            );
            assert_eq!(auth_state, initial_auth, "{initial_error}: {code}");
            assert_eq!(health, initial_health, "{initial_error}: {code}");
            assert_eq!(last_error_code.as_deref(), Some(initial_error));
            assert!(!recover_model_discovery_state(
                &mut auth_state,
                &mut health,
                &mut last_error_code,
            ));
            assert!(!account_candidate_health(
                auth_state,
                health,
                SubscriptionStatus::Active,
                last_error_code.as_deref(),
            )
            .is_eligible());
        }
    }
}

#[test]
fn model_discovery_transient_failure_preserves_terminal_catalog_failure() {
    let mut auth_state = AccountAuthState::Active;
    let mut health = AccountHealthState::Healthy;
    let mut last_error_code = None;
    apply_model_discovery_failure(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
        "models_unauthorized",
        false,
    );
    for (code, retryable) in [("models_transport", true), ("models_forbidden", false)] {
        apply_model_discovery_failure(
            &mut auth_state,
            &mut health,
            &mut last_error_code,
            code,
            retryable,
        );
        assert_eq!(auth_state, AccountAuthState::Error);
        assert_eq!(health, AccountHealthState::Unhealthy);
        assert_eq!(last_error_code.as_deref(), Some("models_unauthorized"));
    }
    assert!(recover_model_discovery_state(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
    ));
    assert_eq!(auth_state, AccountAuthState::Active);
    assert_eq!(health, AccountHealthState::Healthy);
    assert!(last_error_code.is_none());
}

#[test]
fn model_discovery_success_does_not_clear_persisted_block() {
    let mut auth_state = AccountAuthState::Active;
    let mut health = AccountHealthState::Blocked;
    let mut last_error_code = Some("models_forbidden".to_string());
    assert!(!recover_model_discovery_state(
        &mut auth_state,
        &mut health,
        &mut last_error_code,
    ));
    assert_eq!(health, AccountHealthState::Blocked);
    assert_eq!(last_error_code.as_deref(), Some("models_forbidden"));
}

#[test]
fn account_usage_reducer_ignores_request_errors_and_restores_success() {
    let neutral = reduce_account_usage(
        usage_state(),
        AccountUsageObservation {
            success: false,
            http_status: 400,
            error_category: Some("upstream_invalid_request"),
            affects_account: false,
        },
        10,
        None,
        None,
    );
    assert_eq!(neutral.state, usage_state());
    assert!(!neutral.reset_runtime_failures);

    let success = reduce_account_usage(
        AccountUsageState {
            auth_state: AccountAuthState::Error,
            health: AccountHealthState::Unhealthy,
            last_error_code: Some("upstream_unauthorized".into()),
            last_used_at_ms: None,
        },
        AccountUsageObservation {
            success: true,
            http_status: 200,
            error_category: None,
            affects_account: false,
        },
        20,
        None,
        Some(AccountAuthState::Active),
    );
    assert_eq!(
        success.state,
        AccountUsageState {
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            last_error_code: None,
            last_used_at_ms: Some(20),
        }
    );
    assert!(success.reset_runtime_failures);
}

#[test]
fn account_usage_reducer_distinguishes_refreshable_and_access_only_401() {
    let observation = AccountUsageObservation {
        success: false,
        http_status: 401,
        error_category: Some("token_invalidated"),
        affects_account: true,
    };
    let refreshable = reduce_account_usage(
        usage_state(),
        observation,
        10,
        Some(AccountAccessState::Refreshable),
        None,
    );
    assert_eq!(refreshable.state.health, AccountHealthState::Degraded);
    assert_eq!(refreshable.state.auth_state, AccountAuthState::Active);
    assert!(refreshable.refresh_quota);

    let access_only = reduce_account_usage(
        usage_state(),
        observation,
        10,
        Some(AccountAccessState::AccessOnly),
        None,
    );
    assert_eq!(access_only.state.health, AccountHealthState::Unhealthy);
    assert_eq!(access_only.state.auth_state, AccountAuthState::Error);
    assert!(!access_only.refresh_quota);
}

#[test]
fn account_usage_reducer_keeps_quota_and_entitlement_failures_recoverable() {
    for (status, category, refresh_quota, health, error) in [
        (
            403,
            "upstream_quota_exhausted",
            true,
            AccountHealthState::Healthy,
            None,
        ),
        (
            429,
            "upstream_quota_exhausted",
            true,
            AccountHealthState::Healthy,
            None,
        ),
        (
            403,
            "upstream_usage_not_included",
            false,
            AccountHealthState::Degraded,
            Some("upstream_usage_not_included"),
        ),
        (
            429,
            "upstream_rate_limited",
            true,
            AccountHealthState::Degraded,
            Some("upstream_rate_limited"),
        ),
    ] {
        let update = reduce_account_usage(
            usage_state(),
            AccountUsageObservation {
                success: false,
                http_status: status,
                error_category: Some(category),
                affects_account: true,
            },
            10,
            None,
            None,
        );
        assert_eq!(update.state.health, health);
        assert_eq!(update.state.last_error_code.as_deref(), error);
        assert_eq!(update.refresh_quota, refresh_quota);
    }

    let mut stale_quota = usage_state();
    stale_quota.health = AccountHealthState::Degraded;
    stale_quota.last_error_code = Some("upstream_quota_exhausted".into());
    let cleared = reduce_account_usage(
        stale_quota,
        AccountUsageObservation {
            success: false,
            http_status: 403,
            error_category: Some("upstream_quota_exhausted"),
            affects_account: true,
        },
        10,
        None,
        None,
    );
    assert_eq!(cleared.state.health, AccountHealthState::Healthy);
    assert_eq!(cleared.state.last_error_code, None);
    assert!(cleared.refresh_quota);
    assert!(cleared.reset_runtime_failures);

    let mut rate_limited = usage_state();
    rate_limited.health = AccountHealthState::Degraded;
    rate_limited.last_error_code = Some("upstream_rate_limited".into());
    let kept = reduce_account_usage(
        rate_limited,
        AccountUsageObservation {
            success: false,
            http_status: 403,
            error_category: Some("upstream_quota_exhausted"),
            affects_account: true,
        },
        10,
        None,
        None,
    );
    assert_eq!(kept.state.health, AccountHealthState::Degraded);
    assert_eq!(
        kept.state.last_error_code.as_deref(),
        Some("upstream_rate_limited")
    );
    assert!(kept.refresh_quota);
    assert!(!kept.reset_runtime_failures);

    let forbidden = reduce_account_usage(
        usage_state(),
        AccountUsageObservation {
            success: false,
            http_status: 403,
            error_category: Some("upstream_forbidden"),
            affects_account: true,
        },
        10,
        None,
        None,
    );
    assert_eq!(forbidden.state.health, AccountHealthState::Degraded);
    assert_eq!(
        forbidden.state.last_error_code.as_deref(),
        Some("upstream_forbidden")
    );

    for category in ["upstream_account_disabled", "deactivated_workspace"] {
        let blocked = reduce_account_usage(
            usage_state(),
            AccountUsageObservation {
                success: false,
                http_status: 403,
                error_category: Some(category),
                affects_account: true,
            },
            10,
            None,
            None,
        );
        assert_eq!(blocked.state.health, AccountHealthState::Blocked);
        assert_eq!(blocked.state.last_error_code.as_deref(), Some(category));
    }

    let mut disabled = usage_state();
    disabled.health = AccountHealthState::Blocked;
    disabled.last_error_code = Some("deactivated_workspace".into());
    let preserved = reduce_account_usage(
        disabled,
        AccountUsageObservation {
            success: false,
            http_status: 403,
            error_category: Some("upstream_forbidden"),
            affects_account: true,
        },
        10,
        None,
        None,
    );
    assert_eq!(preserved.state.health, AccountHealthState::Blocked);
    assert_eq!(
        preserved.state.last_error_code.as_deref(),
        Some("deactivated_workspace")
    );

    let mut sign_in = usage_state();
    sign_in.auth_state = AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant);
    sign_in.health = AccountHealthState::Unhealthy;
    sign_in.last_error_code = Some("invalid_grant".into());
    let still_signed_out = reduce_account_usage(
        sign_in,
        AccountUsageObservation {
            success: false,
            http_status: 403,
            error_category: Some("upstream_forbidden"),
            affects_account: true,
        },
        10,
        None,
        None,
    );
    assert!(still_signed_out.state.auth_state.requires_fresh_login());
    assert_eq!(still_signed_out.state.health, AccountHealthState::Unhealthy);
    assert_eq!(
        still_signed_out.state.last_error_code.as_deref(),
        Some("invalid_grant")
    );

    let verification = reduce_account_usage(
        usage_state(),
        AccountUsageObservation {
            success: false,
            http_status: 403,
            error_category: Some("upstream_account_verification_required"),
            affects_account: true,
        },
        10,
        None,
        None,
    );
    assert_eq!(verification.state.health, AccountHealthState::Degraded);
    assert_eq!(
        verification.state.last_error_code.as_deref(),
        Some("upstream_account_verification_required")
    );
}

#[test]
fn blank_model_discovery_keeps_the_previous_catalog() {
    let mut models = vec!["gpt-live".into()];
    let mut discovered = Some(vec!["gpt-live".into()]);
    let mut auth = AccountAuthState::Active;
    let mut health = AccountHealthState::Healthy;
    let mut error = None;

    assert!(accept_discovered_models(
        &mut models,
        &mut discovered,
        &mut auth,
        &mut health,
        &mut error,
        Vec::new(),
    ));
    assert_eq!(models, ["gpt-live"]);
    assert_eq!(discovered.as_deref(), Some(models.as_slice()));

    discovered = None;
    assert!(accept_discovered_models(
        &mut models,
        &mut discovered,
        &mut auth,
        &mut health,
        &mut error,
        Vec::new(),
    ));
    assert_eq!(models, ["gpt-live"]);
    assert!(discovered.is_none());

    assert!(accept_discovered_models(
        &mut models,
        &mut discovered,
        &mut auth,
        &mut health,
        &mut error,
        vec!["gpt-next".into()],
    ));
    assert_eq!(models, ["gpt-live"]);
    assert_eq!(
        discovered.as_deref(),
        Some(["gpt-next".to_string()].as_slice())
    );
}

#[test]
fn first_nonempty_model_discovery_fills_an_empty_baseline() {
    let mut models = Vec::new();
    let mut discovered = None;
    let mut auth = AccountAuthState::Error;
    let mut health = AccountHealthState::Unhealthy;
    let mut error = Some("models_transport".into());

    assert!(!accept_discovered_models(
        &mut models,
        &mut discovered,
        &mut auth,
        &mut health,
        &mut error,
        Vec::new(),
    ));
    assert!(models.is_empty());
    assert!(discovered.is_none());
    assert_eq!(error.as_deref(), Some("models_transport"));

    assert!(accept_discovered_models(
        &mut models,
        &mut discovered,
        &mut auth,
        &mut health,
        &mut error,
        vec!["gpt-recovered".into()],
    ));
    assert_eq!(models, ["gpt-recovered"]);
    assert_eq!(
        discovered.as_deref(),
        Some(["gpt-recovered".to_string()].as_slice())
    );
    assert_eq!(auth, AccountAuthState::Active);
    assert_eq!(health, AccountHealthState::Healthy);
    assert!(error.is_none());
}

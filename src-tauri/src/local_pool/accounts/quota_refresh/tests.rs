use super::*;

#[test]
fn manual_refresh_distinguishes_reauth_from_retryable_failures() {
    assert_eq!(
        classify_manual_refresh_failure(TokenRefreshFailureKind::InvalidGrant),
        (
            CredentialRefreshStatus::RequiresReauth,
            Some(ReauthReason::InvalidGrant)
        )
    );
    assert_eq!(
        classify_manual_refresh_failure(TokenRefreshFailureKind::ExpiredRefreshToken),
        (
            CredentialRefreshStatus::RequiresReauth,
            Some(ReauthReason::ExpiredRefreshToken)
        )
    );
    assert_eq!(
        classify_manual_refresh_failure(TokenRefreshFailureKind::InvalidatedRefreshToken),
        (
            CredentialRefreshStatus::RequiresReauth,
            Some(ReauthReason::InvalidatedRefreshToken)
        )
    );
    assert_eq!(
        classify_manual_refresh_failure(TokenRefreshFailureKind::ReusedRefreshToken),
        (CredentialRefreshStatus::RetryableFailure, None)
    );
    assert_eq!(
        classify_manual_refresh_failure(TokenRefreshFailureKind::Transient),
        (CredentialRefreshStatus::RetryableFailure, None)
    );
}

#[test]
fn successful_refresh_only_clears_credential_owned_error_codes() {
    assert!(is_credential_refresh_error_code("invalid_grant"));
    assert!(is_credential_refresh_error_code("auth_invalid_grant"));
    assert!(is_credential_refresh_error_code("token_invalidated"));
    assert!(!is_credential_refresh_error_code("models_unauthorized"));
    assert!(!is_credential_refresh_error_code("quota_exhausted"));
}

#[test]
fn stale_manual_refresh_failure_never_wins_over_a_newer_token_snapshot() {
    let failed = TokenSet::new("access", Some("refresh".into()), None, None, 100, 7).unwrap();

    assert!(!persisted_token_generation_is_newer(7, Some(100), &failed));
    assert!(persisted_token_generation_is_newer(8, Some(1), &failed));
    assert!(persisted_token_generation_is_newer(7, Some(101), &failed));
    assert!(!persisted_token_generation_is_newer(6, Some(999), &failed));
}

#[test]
fn model_refresh_preparation_errors_have_stable_codes() {
    assert_eq!(
        model_refresh_error_kind(ErrorCode::GatewayUnavailable),
        Some(("models_proxy_unavailable", true))
    );
    assert_eq!(
        model_refresh_error_kind(ErrorCode::SecretStoreUnavailable),
        Some(("models_secret_store", true))
    );
    assert_eq!(
        model_refresh_error_kind(ErrorCode::ProfileRestoreBlocked),
        Some(("models_profile_restore", false))
    );
    assert_eq!(model_refresh_error_kind(ErrorCode::NotFound), None);
}

#[test]
fn authorization_debug_and_refresh_cache_never_expose_prepared_secrets() {
    let prepared = PreparedAccountAuthorization {
        authorization: bearer_authorization("synthetic-unique-access").unwrap(),
        subscription_authorization: None,
        tokens: None,
        agent_task_id: Some("synthetic-unique-task".into()),
        provider_account_id: "synthetic-unique-provider".into(),
        proxy: None,
    };
    let value = Ok(RefreshRead::Authorization(Box::new(prepared)));
    let debug = format!("{value:?}");
    for secret in [
        "synthetic-unique-access",
        "synthetic-unique-task",
        "synthetic-unique-provider",
    ] {
        assert!(!debug.contains(secret));
    }
    assert!(!refresh::cache_observation(&value));
}

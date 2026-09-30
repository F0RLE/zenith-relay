use super::*;
use std::sync::atomic::Ordering;

struct InvalidGrant;

impl TokenRefreshAdapter for InvalidGrant {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async {
            Err(TokenRefreshFailure::new(
                TokenRefreshFailureKind::InvalidGrant,
                "invalid_grant",
            ))
        })
    }
}

#[tokio::test]
async fn invalid_grant_marks_account_requires_reauth() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "account",
            TokenSet::new("access", Some("refresh".into()), None, Some(1), 0, 0).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();

    assert!(matches!(
        authority.prepare("account", 2, 0, &InvalidGrant).await,
        Err(TokenAuthorityError::RequiresReauth(
            ReauthReason::InvalidGrant
        ))
    ));
    assert_eq!(
        authority.auth_state("account").await,
        Some(AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant))
    );
}

#[tokio::test]
async fn terminal_auth_state_is_persisted_without_token_material() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "local-account",
            TokenSet::new("access", Some("refresh".into()), None, Some(1), 0, 0).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let persistence = CapturePersistence::default();

    assert!(matches!(
        authority
            .prepare_and_persist("local-account", 2, 0, &InvalidGrant, &persistence)
            .await,
        Err(TokenAuthorityError::RequiresReauth(
            ReauthReason::InvalidGrant
        ))
    ));
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        *persistence.auth_states.lock().unwrap(),
        vec![(
            "local-account".to_string(),
            AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant)
        )]
    );
}

struct TransientRefreshFailure;

impl TokenRefreshAdapter for TransientRefreshFailure {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async {
            Err(TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                "transport",
            ))
        })
    }
}

struct ReusedRefreshFailure;

impl TokenRefreshAdapter for ReusedRefreshFailure {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async {
            Err(TokenRefreshFailure::new(
                TokenRefreshFailureKind::ReusedRefreshToken,
                "refresh_token_reused",
            ))
        })
    }
}

async fn active_authority_with_refreshable_expired_token() -> TokenAuthority {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "local-account",
            TokenSet::new(
                "access",
                Some("refresh".into()),
                Some("identity".into()),
                Some(1),
                0,
                7,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    authority
}

#[tokio::test]
async fn transient_refresh_failure_preserves_auth_state_and_tokens() {
    let authority = active_authority_with_refreshable_expired_token().await;
    let persistence = CapturePersistence::default();

    assert_eq!(
        authority
            .prepare_and_persist(
                "local-account",
                2,
                0,
                &TransientRefreshFailure,
                &persistence,
            )
            .await
            .unwrap_err(),
        TokenAuthorityError::RefreshFailed("transport".into())
    );
    assert_eq!(
        authority.auth_state("local-account").await,
        Some(AccountAuthState::Active)
    );
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
    assert!(persistence.auth_states.lock().unwrap().is_empty());

    let tokens = authority.tokens("local-account").await.unwrap();
    assert_eq!(tokens.access_token(), "access");
    assert_eq!(tokens.refresh_token(), Some("refresh"));
    assert_eq!(tokens.id_token(), Some("identity"));
    assert_eq!(tokens.expires_at_ms(), Some(1));
    assert_eq!(tokens.issued_at_ms(), 0);
    assert_eq!(tokens.generation(), 7);
}

#[tokio::test]
async fn reused_refresh_token_preserves_auth_state_and_tokens() {
    let authority = active_authority_with_refreshable_expired_token().await;
    let persistence = CapturePersistence::default();

    assert_eq!(
        authority
            .prepare_and_persist("local-account", 2, 0, &ReusedRefreshFailure, &persistence,)
            .await
            .unwrap_err(),
        TokenAuthorityError::RefreshFailed("refresh_token_reused".into())
    );
    assert_eq!(
        authority.auth_state("local-account").await,
        Some(AccountAuthState::Active)
    );
    assert!(persistence.auth_states.lock().unwrap().is_empty());
    assert_eq!(
        authority
            .tokens("local-account")
            .await
            .unwrap()
            .generation(),
        7
    );
}

#[tokio::test]
async fn legacy_reused_refresh_token_reauth_state_is_healed_before_prepare() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "local-account",
            TokenSet::new(
                "access",
                Some("refresh".into()),
                Some("identity".into()),
                Some(10_000),
                0,
                7,
            )
            .unwrap(),
            AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken),
        )
        .await
        .unwrap();

    let prepared = authority
        .prepare("local-account", 1, 0, &TransientRefreshFailure)
        .await
        .expect("a legacy transient state must not force login");

    assert_eq!(prepared.status, PrepareStatus::Ready);
    assert_eq!(
        authority.auth_state("local-account").await,
        Some(AccountAuthState::Active)
    );
}

#[tokio::test]
async fn invalidated_access_is_expired_and_persisted_once() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "local-account",
            TokenSet::new("access", Some("refresh".into()), None, Some(60_000), 1, 7).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let persistence = CapturePersistence::default();

    authority
        .invalidate_access_and_persist("local-account", 10, &persistence)
        .await
        .unwrap();

    let tokens = authority.tokens("local-account").await.unwrap();
    assert_eq!(tokens.expires_at_ms(), Some(10));
    assert_eq!(tokens.generation(), 8);
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_management_unauthorized_does_not_invalidate_a_newer_login() {
    let authority = TokenAuthority::new(1).unwrap();
    let current = TokenSet::new(
        "synthetic-new-access",
        Some("synthetic-new-refresh".into()),
        None,
        Some(60_000),
        2,
        8,
    )
    .unwrap();
    authority
        .register("local-account", current.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let persistence = CapturePersistence::default();
    assert!(!authority
        .invalidate_access_generation_and_persist("local-account", Some(7), 10, &persistence,)
        .await
        .unwrap());
    assert_eq!(
        authority.tokens("local-account").await,
        Some(current.clone())
    );
    assert_eq!(
        authority.auth_state("local-account").await,
        Some(AccountAuthState::Active)
    );
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);

    let same_generation_new_login = TokenSet::new(
        "another-login",
        Some("another-refresh".into()),
        None,
        Some(60_000),
        2,
        8,
    )
    .unwrap();
    assert!(!authority
        .invalidate_access_if_current_and_persist(
            "local-account",
            &same_generation_new_login,
            10,
            &persistence
        )
        .await
        .unwrap());
    assert_eq!(
        authority.tokens("local-account").await,
        Some(current.clone())
    );
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);

    assert!(authority
        .invalidate_access_generation_and_persist("local-account", Some(8), 20, &persistence,)
        .await
        .unwrap());
    assert!(!authority
        .invalidate_access_generation_and_persist("local-account", Some(8), 30, &persistence,)
        .await
        .unwrap());
    assert_eq!(
        authority
            .tokens("local-account")
            .await
            .unwrap()
            .generation(),
        9
    );
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 1);
}

struct MustNotRefresh;

impl TokenRefreshAdapter for MustNotRefresh {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async { panic!("access-only token must not refresh") })
    }
}

#[tokio::test]
async fn expired_access_only_token_never_attempts_refresh() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "account",
            TokenSet::access_only("access", Some(1), 0).unwrap(),
            AccountAuthState::DegradedAccessOnly,
        )
        .await
        .unwrap();

    assert!(matches!(
        authority.prepare("account", 2, 0, &MustNotRefresh).await,
        Err(TokenAuthorityError::AccessTokenExpired)
    ));
}

use super::*;

async fn prepared(fixture: &Fixture) -> PreparedAccountAuthorization {
    PreparedAccountAuthorization::from_tokens(
        prepare_account_credentials(&fixture.state, fixture.saved.local_account_id())
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn late_siwc_401_keeps_reinstalled_and_already_rotated_sessions_intact() {
    for (reinstall, identical) in [(true, true), (true, false), (false, false)] {
        let now_ms = current_time_ms();
        let fixture =
            Fixture::access_only(&["resource.invoke", "chatgpt.tokens.use.direct"], now_ms);
        let id = fixture.saved.local_account_id();
        let rejected = prepared(&fixture).await;
        let tokens = if identical {
            fixture.saved.to_token_set().unwrap()
        } else {
            TokenSet::new(
                "synthetic-replacement-access",
                None,
                None,
                Some(now_ms + 3_600_000),
                now_ms,
                if reinstall { 2 } else { 3 },
            )
            .unwrap()
        };
        let replacement = fixture.saved.with_token_set(&tokens).unwrap();
        fixture.store.save(&replacement).unwrap();
        let authority = fixture.state.token_authority();
        if reinstall {
            authority
                .register(id, tokens.clone(), AccountAuthState::Active)
                .await
                .unwrap();
        } else {
            authority
                .register_if_newer(id, tokens.clone(), AccountAuthState::Active)
                .await
                .unwrap();
        }
        let recovered = recover_account_authorization(&fixture.state, id, &rejected, now_ms).await;
        let saved = fixture.store.require(id).unwrap();
        let cached = authority.tokens(id).await;
        fixture.close();

        assert_eq!(recovered.unwrap().tokens, tokens);
        assert_eq!(cached, Some(tokens));
        assert!(saved.matches_snapshot(&replacement));
    }
}

#[tokio::test]
async fn current_siwc_401_expires_only_the_rejected_access_only_session() {
    let now_ms = current_time_ms();
    let fixture = Fixture::access_only(&["resource.invoke", "chatgpt.tokens.use.direct"], now_ms);
    let id = fixture.saved.local_account_id();
    let tokens = TokenSet::new(
        "synthetic-access-only",
        None,
        None,
        Some(now_ms + 3_600_000),
        now_ms,
        2,
    )
    .unwrap();
    let access_only = fixture.saved.with_token_set(&tokens).unwrap();
    assert!(access_only.refresh_token().is_none());
    fixture.store.save(&access_only).unwrap();
    let rejected = prepared(&fixture).await;
    let result = recover_account_authorization(&fixture.state, id, &rejected, now_ms).await;
    let current = fixture.state.token_authority().tokens(id).await.unwrap();
    let saved = fixture.store.require(id).unwrap();
    fixture.close();

    let error = result.unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidState);
    assert_eq!(
        error.message,
        "account access token expired and cannot be refreshed"
    );
    assert_eq!(current.access_token(), tokens.access_token());
    assert_eq!(current.generation(), tokens.generation() + 1);
    assert!(!current.is_access_usable(now_ms, 0));
    assert_eq!(saved.to_token_set().unwrap(), current);
    assert_eq!(saved.refresh_token(), None);
    assert_eq!(saved.access_token(), tokens.access_token());
}

#[tokio::test]
async fn siwc_recovery_without_the_original_revision_has_no_side_effects() {
    let now_ms = current_time_ms();
    let fixture = Fixture::access_only(&["resource.invoke", "chatgpt.tokens.use.direct"], now_ms);
    let id = fixture.saved.local_account_id();
    let mut rejected = prepared(&fixture).await;
    let before = fixture.state.token_authority().tokens(id).await;
    rejected.token_revision = None;
    let result = recover_account_authorization(&fixture.state, id, &rejected, now_ms).await;
    let after = fixture.state.token_authority().tokens(id).await;
    let unchanged = fixture
        .store
        .require(id)
        .unwrap()
        .matches_snapshot(&fixture.saved);
    fixture.close();

    assert_eq!(result.unwrap_err().code, ErrorCode::InvalidState);
    assert_eq!(after, before);
    assert!(unchanged);
}

#[tokio::test]
async fn stale_siwc_401_cannot_restore_plan_permission_from_a_previous_login() {
    let now_ms = current_time_ms();
    let fixture = Fixture::access_only(&["resource.invoke", "chatgpt.tokens.use.direct"], now_ms);
    let id = fixture.saved.local_account_id();
    let rejected = prepared(&fixture).await;
    let mut registration = fixture.saved.siwc_registration().unwrap().clone();
    registration.scopes = vec!["openid".into()];
    let replacement = fixture
        .saved
        .clone()
        .with_siwc_registration(Some(registration))
        .unwrap();
    fixture.store.save(&replacement).unwrap();
    let tokens = replacement.to_token_set().unwrap();
    let authority = fixture.state.token_authority();
    authority
        .register(id, tokens.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let result = recover_account_authorization(&fixture.state, id, &rejected, now_ms).await;
    let cached = authority.tokens(id).await;
    let saved = fixture.store.require(id).unwrap();
    fixture.close();

    let error = result.unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidState);
    assert!(error.message.contains("ChatGPT plan use is disabled"));
    assert_eq!(cached, Some(tokens));
    assert!(saved.matches_snapshot(&replacement));
}

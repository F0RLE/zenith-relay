use super::*;

#[tokio::test]
async fn usage_observation_keeps_the_current_slot_fenced_without_waiting_for_refresh() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register_if_absent(
            "account",
            TokenSet::new("synthetic-access", None, None, None, 1, 2).unwrap(),
            AccountAuthState::Active,
        )
        .unwrap();
    let entry = { lock(&authority.slots).get("account").cloned().unwrap() };
    let busy = entry.lock().await;
    assert_eq!(authority.with_usage_observation("account", 2, |_| ()), None);
    drop(busy);
    assert_eq!(authority.with_usage_observation("account", 1, |_| ()), None);
    assert_eq!(
        authority.with_usage_observation("account", 2, |state| {
            assert!(entry.try_lock().is_none());
            assert!(entry.revision.try_write().is_err());
            state
        }),
        Some(AccountAuthState::Active)
    );
    assert!(entry.try_lock().is_some());
    assert!(entry.revision.try_write().is_ok());
    assert!(authority.remove("account"));
    assert_eq!(authority.with_usage_observation("account", 2, |_| ()), None);
}

#[tokio::test]
async fn register_if_absent_never_overwrites_newer_tokens() {
    let authority = TokenAuthority::new(1).unwrap();
    let first = TokenSet::new("new-access", Some("new-refresh".into()), None, None, 2, 2).unwrap();
    let stale = TokenSet::new("old-access", Some("old-refresh".into()), None, None, 1, 1).unwrap();

    assert!(
        authority
            .register_if_absent("account", first, AccountAuthState::Active)
            .unwrap()
    );
    assert!(
        !authority
            .register_if_absent("account", stale, AccountAuthState::Active)
            .unwrap()
    );

    let stored = authority.tokens("account").await.unwrap();
    assert_eq!(stored.generation(), 2);
    assert_eq!(stored.access_token(), "new-access");
}

#[tokio::test]
async fn register_if_absent_does_not_wait_for_an_owned_slot() {
    let authority = TokenAuthority::new(1).unwrap();
    let current = TokenSet::new(
        "current-access",
        Some("current-refresh".into()),
        None,
        None,
        2,
        2,
    )
    .unwrap();
    assert!(
        authority
            .register_if_absent("account", current.clone(), AccountAuthState::Active)
            .unwrap()
    );
    let slot = { lock(&authority.slots).get("account").cloned().unwrap() };
    // An OAuth refresh holds this guard while waiting on the provider. A
    // runtime rebuild must finish without waiting for that unrelated I/O.
    let refresh_guard = slot.lock().await;
    let snapshot =
        TokenSet::new("disk-access", Some("disk-refresh".into()), None, None, 3, 3).unwrap();
    assert!(
        !authority
            .register_if_absent("account", snapshot, AccountAuthState::Active)
            .unwrap()
    );
    assert_eq!(refresh_guard.tokens, current);
}

#[tokio::test]
async fn conditional_registration_never_replaces_a_newer_refresh_generation() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "account",
            TokenSet::new("new-access", Some("new-refresh".into()), None, None, 2, 2).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();

    let stale = TokenSet::new("old-access", Some("old-refresh".into()), None, None, 1, 1).unwrap();
    assert!(
        !authority
            .register_if_not_stale("account", stale, AccountAuthState::Active)
            .await
            .unwrap()
    );
    assert_eq!(
        authority.tokens("account").await.unwrap().access_token(),
        "new-access"
    );

    let newest = TokenSet::new(
        "newest-access",
        Some("newest-refresh".into()),
        None,
        None,
        3,
        3,
    )
    .unwrap();
    assert!(
        authority
            .register_if_not_stale("account", newest, AccountAuthState::Active)
            .await
            .unwrap()
    );
    assert_eq!(
        authority.tokens("account").await.unwrap().access_token(),
        "newest-access"
    );
}

#[tokio::test]
async fn conditional_rollback_never_replaces_a_newer_token_generation() {
    let authority = TokenAuthority::new(1).unwrap();
    let attempted = TokenSet::new(
        "attempted-access",
        Some("attempted-refresh".into()),
        None,
        Some(20_000),
        20,
        2,
    )
    .unwrap();
    let newer = TokenSet::new(
        "newer-access",
        Some("newer-refresh".into()),
        None,
        Some(30_000),
        30,
        3,
    )
    .unwrap();
    let previous = TokenSet::new(
        "previous-access",
        Some("previous-refresh".into()),
        None,
        Some(10_000),
        10,
        1,
    )
    .unwrap();
    authority
        .register("account", newer, AccountAuthState::Active)
        .await
        .unwrap();

    assert!(
        !authority
            .replace_if_current(
                "account",
                &attempted,
                AccountAuthState::Active,
                previous,
                AccountAuthState::Active,
            )
            .await
            .unwrap()
    );
    assert_eq!(
        authority.tokens("account").await.unwrap().access_token(),
        "newer-access"
    );
}

#[tokio::test]
async fn conditional_remove_never_evicts_a_reused_account_slot() {
    let authority = TokenAuthority::new(1).unwrap();
    let attempted = TokenSet::new(
        "attempted-access",
        Some("attempted-refresh".into()),
        None,
        Some(20_000),
        20,
        2,
    )
    .unwrap();
    let newer = TokenSet::new(
        "newer-access",
        Some("newer-refresh".into()),
        None,
        Some(30_000),
        30,
        3,
    )
    .unwrap();
    authority
        .register("account", newer, AccountAuthState::Active)
        .await
        .unwrap();

    assert!(
        !authority
            .remove_if_current("account", &attempted, AccountAuthState::Active)
            .await
            .unwrap()
    );
    assert_eq!(authority.len(), 1);
    assert_eq!(
        authority.tokens("account").await.unwrap().access_token(),
        "newer-access"
    );
}

#[tokio::test]
async fn newer_only_registration_preserves_equal_generation_auth_state() {
    let authority = TokenAuthority::new(1).unwrap();
    let current = TokenSet::new(
        "access",
        Some("refresh".into()),
        Some("identity".into()),
        Some(10_000),
        7,
        3,
    )
    .unwrap();
    authority
        .register(
            "account",
            current.clone(),
            AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
        )
        .await
        .unwrap();

    assert!(
        !authority
            .register_if_newer("account", current, AccountAuthState::Active)
            .await
            .unwrap()
    );
    assert_eq!(
        authority.auth_state("account").await,
        Some(AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant))
    );

    let newer = TokenSet::new(
        "new-access",
        Some("new-refresh".into()),
        Some("new-identity".into()),
        Some(20_000),
        8,
        3,
    )
    .unwrap();
    assert!(
        authority
            .register_if_newer("account", newer, AccountAuthState::Active)
            .await
            .unwrap()
    );
    assert_eq!(
        authority.auth_state("account").await,
        Some(AccountAuthState::Active)
    );
}

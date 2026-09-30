use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn twenty_concurrent_prepares_rotate_once() {
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "account",
            TokenSet::new(
                "old-access-secret",
                Some("refresh-secret".to_string()),
                Some("id-secret".to_string()),
                Some(1),
                0,
                7,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let adapter = Arc::new(RefreshOnce {
        calls: AtomicUsize::new(0),
    });
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let authority = authority.clone();
        let adapter = adapter.clone();
        tasks.push(tokio::spawn(async move {
            authority.prepare("account", 10, 0, adapter.as_ref()).await
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap().unwrap());
    }

    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result.status == PrepareStatus::Refreshed)
            .count(),
        1
    );
    let tokens = authority.tokens("account").await.unwrap();
    assert_eq!(tokens.generation(), 8);
    assert_eq!(tokens.refresh_token(), Some("refresh-secret"));
    let debug = format!("{tokens:?}");
    assert!(!debug.contains("new-access-secret"));
    assert!(!debug.contains("refresh-secret"));
    assert!(!debug.contains("id-secret"));
}

#[tokio::test]
async fn prepared_token_revision_rejects_refresh_invalidation_and_readded_slot() {
    let authority = TokenAuthority::new(1).unwrap();
    let original = TokenSet::new(
        "old-access-secret",
        Some("refresh-secret".into()),
        None,
        Some(100),
        0,
        7,
    )
    .unwrap();
    authority
        .register("account", original.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let adapter = RefreshOnce {
        calls: AtomicUsize::new(0),
    };
    let before = authority.prepare("account", 10, 0, &adapter).await.unwrap();
    assert!(before.dispatch_revision.guard().is_some());
    let refreshed = authority
        .prepare("account", 101, 0, &adapter)
        .await
        .unwrap();
    assert!(before.dispatch_revision.guard().is_none());
    assert!(refreshed.dispatch_revision.guard().is_some());

    let persistence = CapturePersistence::default();
    authority
        .invalidate_access_and_persist("account", 102, &persistence)
        .await
        .unwrap();
    assert!(refreshed.dispatch_revision.guard().is_none());

    let after_invalidation = authority
        .tokens("account")
        .await
        .expect("invalidated slot remains stored");
    assert!(authority.remove("account"));
    authority
        .register("account", after_invalidation, AccountAuthState::Active)
        .await
        .unwrap();
    assert!(refreshed.dispatch_revision.guard().is_none());
    assert!(before.dispatch_revision.guard().is_none());
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
}

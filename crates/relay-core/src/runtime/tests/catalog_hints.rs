use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
async fn native_catalog_hints_retry_rejected_callbacks_and_fence_replaced_credentials() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let authority = &runtime.chatgpt_accounts["account-1"].token_authority;
    let token = TokenSet::access_only("synthetic-first", None, 1).unwrap();
    authority
        .register("account-1", token.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let prepared = runtime.prepare_authorization("account-1", 1).await.unwrap();
    let accepted = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let enabled = accepted.clone();
    let calls = attempts.clone();
    runtime.set_codex_catalog_hint_callback(move |account, delay| {
        calls.lock().unwrap().push((account, delay));
        enabled.load(Ordering::Acquire)
    });
    let url = Url::parse("https://chatgpt.com/backend-api/codex/responses").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-models-etag", HeaderValue::from_static("v1"));
    runtime.observe_codex_models_etag("account-1", &url, &headers, &prepared);
    assert!(crate::poison::mutex(&runtime.codex_catalog_hints).is_empty());
    accepted.store(true, Ordering::Release);
    runtime.observe_codex_models_etag("account-1", &url, &headers, &prepared);
    runtime.observe_codex_models_etag("account-1", &url, &headers, &prepared);
    assert_eq!(attempts.lock().unwrap().len(), 2);
    assert_eq!(attempts.lock().unwrap()[1], ("account-1".to_string(), 0));

    // A replacement registration fences the old response, even for identical tokens.
    authority
        .register("account-1", token, AccountAuthState::Active)
        .await
        .unwrap();
    headers.insert("x-models-etag", HeaderValue::from_static("v2"));
    runtime.observe_codex_models_etag("account-1", &url, &headers, &prepared);
    assert_eq!(attempts.lock().unwrap().len(), 2);
    let replacement = runtime.prepare_authorization("account-1", 1).await.unwrap();
    runtime.observe_codex_models_etag("account-1", &url, &headers, &replacement);
    assert_eq!(attempts.lock().unwrap().len(), 3);

    authority
        .register(
            "account-1",
            TokenSet::access_only("synthetic-second", None, 2).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let replacement = runtime.prepare_authorization("account-1", 1).await.unwrap();
    runtime.observe_codex_models_etag("account-1", &url, &headers, &replacement);
    assert_eq!(attempts.lock().unwrap().len(), 4);
    assert_eq!(attempts.lock().unwrap()[3].1, 0);
}

#[tokio::test]
async fn catalog_hints_ignore_non_codex_accounts_and_untrusted_origins() {
    let mut runtime = quota_runtime(QuotaSnapshot::default());
    runtime.chatgpt_accounts["account-1"]
        .token_authority
        .register(
            "account-1",
            TokenSet::access_only("synthetic-access", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let prepared = runtime.prepare_authorization("account-1", 1).await.unwrap();
    runtime.set_codex_catalog_hint_callback(|_, _| panic!("must not refresh"));
    let mut headers = HeaderMap::new();
    headers.insert("x-models-etag", HeaderValue::from_static("v1"));
    runtime.observe_codex_models_etag(
        "account-1",
        &Url::parse("https://example.test/backend-api/codex/responses").unwrap(),
        &headers,
        &prepared,
    );
    let url = Url::parse("https://chatgpt.com/backend-api/codex/responses").unwrap();
    runtime.observe_codex_models_etag("not-an-account", &url, &headers, &prepared);
    for kind in [
        crate::providers::chatgpt::OAuthClientKind::Siwc,
        crate::providers::chatgpt::OAuthClientKind::ExcelBps,
    ] {
        runtime
            .chatgpt_accounts
            .get_mut("account-1")
            .unwrap()
            .oauth_client_kind = kind;
        runtime.observe_codex_models_etag("account-1", &url, &headers, &prepared);
    }
    assert!(crate::poison::mutex(&runtime.codex_catalog_hints).is_empty());
}

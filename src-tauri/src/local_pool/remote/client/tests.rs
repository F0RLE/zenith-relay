use super::profile::validate_profile_credential;
use super::*;
use axum::{
    http::{header::AUTHORIZATION, Uri},
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::Notify;
use zenith_relay_core::protocol::UsageQuery;

#[tokio::test]
async fn routing_conflicts_are_typed_without_exposing_remote_messages() {
    for (status, code, typed) in [
        (400, "pool_routing_conflict", true),
        (409, "pool_routing_conflict", true),
        (400, "invalid_request", false),
        (503, "pool_routing_conflict", false),
    ] {
        let server = spawn(Router::new().route(
            "/routing/settings",
            post(move || async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    Json(serde_json::json!({
                        "error": { "code": code, "message": "synthetic private server detail" }
                    })),
                )
            }),
        ))
        .await;
        let client = RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap();
        let error = client
            .mutate(Method::POST, "/routing/settings", None)
            .await
            .unwrap_err();
        assert_eq!(
            matches!(error, RemoteClientError::PoolRoutingConflict),
            typed
        );
        assert!(!error.to_string().contains("synthetic private"));
    }
}

#[tokio::test]
async fn redirect_is_not_followed_and_token_never_reaches_other_origin() {
    let received = Arc::new(AtomicUsize::new(0));
    let observed = received.clone();
    let target = spawn(Router::new().route(
        "/state",
        get(move |headers: axum::http::HeaderMap| {
            let observed = observed.clone();
            async move {
                if headers.get(AUTHORIZATION).is_some() {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
                "{}"
            }
        }),
    ))
    .await;
    let redirect_target = format!("{target}/state");
    let source = spawn(Router::new().route(
        "/state",
        get(move || {
            let redirect_target = redirect_target.clone();
            async move { Redirect::temporary(&redirect_target) }
        }),
    ))
    .await;
    let client = RemoteClient::new(&source, "synthetic-management-token-value", false).unwrap();
    assert!(matches!(
        client.state().await,
        Err(RemoteClientError::RedirectRejected)
    ));
    assert_eq!(received.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn usage_query_values_are_encoded_and_cannot_add_parameters() {
    let observed = Arc::new(Mutex::new(String::new()));
    let request_uri = observed.clone();
    let server = spawn(Router::new().route(
        "/usage",
        get(move |uri: Uri| {
            let request_uri = request_uri.clone();
            async move {
                *request_uri.lock().unwrap() = uri.to_string();
                Json(serde_json::json!({
                    "events": [], "total": 0, "page": 1, "pageSize": 25, "totalPages": 0
                }))
            }
        }),
    ))
    .await;
    let client = RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap();
    client
        .usage(&UsageQuery {
            page: 1,
            page_size: 25,
            bucket_ms: Some(60_000),
            model_query: Some("gpt test&success=false".to_string()),
            success: Some(true),
            ..UsageQuery::default()
        })
        .await
        .unwrap();
    let uri = observed.lock().unwrap().clone();
    assert!(uri.contains("modelQuery=gpt+test%26success%3Dfalse"));
    assert!(uri.contains("success=true"));
    assert!(uri.contains("bucketMs=60000"));
    assert!(!uri.contains("modelQuery=gpt+test&success=false"));
}

#[tokio::test]
async fn in_flight_request_finishes_after_external_client_owner_is_dropped() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let handler_entered = entered.clone();
    let handler_release = release.clone();
    let server = spawn(Router::new().route(
        "/usage",
        get(move || {
            let entered = handler_entered.clone();
            let release = handler_release.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                Json(serde_json::json!({
                    "events": [], "total": 0, "page": 1, "pageSize": 25, "totalPages": 0
                }))
            }
        }),
    ))
    .await;
    let owner =
        Arc::new(RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap());
    let request_client = owner.clone();
    let request = tokio::spawn(async move { request_client.usage(&UsageQuery::default()).await });

    entered.notified().await;
    drop(owner);
    release.notify_one();

    assert_eq!(request.await.unwrap().unwrap().total, 0);
}

#[tokio::test]
async fn identity_reveal_rejects_a_mismatched_account() {
    let server = spawn(Router::new().route(
        "/accounts/{id}/identity/reveal",
        post(|| async {
            Json(serde_json::json!({
                "accountId": "different-account",
                "identity": "private@example.test"
            }))
        }),
    ))
    .await;
    let client = RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap();
    assert!(matches!(
        client.reveal_account_identity("account-1").await,
        Err(RemoteClientError::InvalidResponse)
    ));
}

#[test]
fn profile_credential_cannot_redirect_codex_to_another_origin() {
    let origin = PinnedOrigin::parse("https://relay.example.test", false).unwrap();
    let credential = RemoteProfileCredential {
        key_id: "key_system".to_string(),
        base_url: "https://other.example.test/v1".to_string(),
        secret: format!("zrs_{}", "a".repeat(40)),
    };
    assert!(matches!(
        validate_profile_credential(&origin, credential),
        Err(RemoteClientError::InvalidResponse)
    ));
}

async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

use super::parse::parse_identity_claims;
use super::*;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::json;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use url::Url;
use zenith_relay_core::accounts::TokenRefreshFailureKind;

#[test]
fn authorization_url_uses_s256_and_callback_state_is_strict() {
    let client = CodexOAuthClient::new().unwrap();
    let start = client.begin(1455, 10_000).unwrap();
    let query: HashMap<_, _> = start
        .authorization_url()
        .query_pairs()
        .into_owned()
        .collect();

    assert_eq!(
        start.authorization_url().as_str().split('?').next(),
        Some("https://auth.openai.com/oauth/authorize")
    );
    assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
    assert_eq!(
        query.get("client_id").map(String::as_str),
        Some(CODEX_OAUTH_CLIENT_ID)
    );
    assert_eq!(
        query.get("scope").map(String::as_str),
        Some(CODEX_OAUTH_SCOPE)
    );
    assert_eq!(
        query.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(
        query.get("id_token_add_organizations").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        query.get("codex_cli_simplified_flow").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        query.get("originator").map(String::as_str),
        Some(CODEX_OAUTH_ORIGINATOR)
    );
    assert!(client.begin(1457, 10_000).is_ok());
    assert_eq!(
        client.begin(1456, 10_000).err().unwrap().code,
        OAuthErrorCode::InvalidCallbackPort
    );
    let expected_challenge =
        URL_SAFE_NO_PAD.encode(Sha256::digest(start.pending.code_verifier.as_bytes()));
    assert_eq!(
        query.get("code_challenge").map(String::as_str),
        Some(expected_challenge.as_str())
    );

    let callback = format!(
        "{}?code=authorization-secret&state={}",
        start.pending.redirect_uri, start.pending.state
    );
    assert!(start.pending().parse_callback(&callback, 10_001).is_ok());
    let error = start
        .pending()
        .parse_callback(
            "http://localhost:1455/auth/callback?code=authorization-secret&state=wrong-secret",
            10_001,
        )
        .unwrap_err();
    assert_eq!(error.code, OAuthErrorCode::StateMismatch);
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("authorization-secret"));
    assert!(!rendered.contains("wrong-secret"));
    assert_eq!(
        start
            .pending()
            .parse_callback(&callback, start.pending().expires_at_ms() + 1)
            .unwrap_err()
            .code,
        OAuthErrorCode::ExpiredCallback
    );
}

#[tokio::test]
async fn code_exchange_uses_form_fields_and_extracts_bounded_claims() {
    let (base_url, server) = spawn(Router::new().route("/oauth/token", post(exchange))).await;
    let client = test_client(&base_url);
    let start = client.begin(1455, 1_000).unwrap();
    let callback = start
        .pending
        .parse_callback(
            &format!(
                "{}?code=authorization-code&state={}",
                start.pending.redirect_uri, start.pending.state
            ),
            1_001,
        )
        .unwrap();
    let tokens = client
        .exchange_code(&start.pending, callback, 2_000)
        .await
        .unwrap();

    assert_eq!(tokens.access_token(), "access-token");
    assert_eq!(tokens.refresh_token(), Some("refresh-token"));
    assert_eq!(tokens.expires_at_ms(), Some(3_602_000));
    let claims = tokens.identity_claims().unwrap().unwrap();
    assert_eq!(claims.email(), Some("user@example.test"));
    assert_eq!(claims.plan_type(), Some("pro"));
    assert_eq!(
        claims.subscription_active_until_ms(),
        Some(1_788_998_400_000)
    );
    assert_eq!(claims.account_id(), Some("account-123"));
    assert_eq!(claims.user_id(), Some("user-123"));
    let rendered = format!("{tokens:?} {claims:?}");
    assert!(!rendered.contains("access-token"));
    assert!(!rendered.contains("refresh-token"));
    assert!(!rendered.contains("user@example.test"));
    assert!(!rendered.contains("account-123"));
    server.abort();
}

#[tokio::test]
async fn refresh_uses_json_and_classifies_reauth_without_secret_leaks() {
    let (base_url, server) = spawn(
        Router::new()
            .route("/oauth/token", post(refresh_without_rotation))
            .route("/oauth/fail", post(refresh_failure)),
    )
    .await;
    let client = test_client(&base_url);
    let refreshed = client
        .exchange_refresh_token("refresh-secret", 5_000)
        .await
        .unwrap();
    assert_eq!(refreshed.access_token(), "new-access-token");
    assert_eq!(refreshed.refresh_token(), None);
    assert_eq!(refreshed.id_token(), None);

    let failing = CodexOAuthClient::with_endpoints(
        base_url.join("oauth/authorize").unwrap(),
        base_url.join("oauth/fail").unwrap(),
    )
    .unwrap();
    let failure = failing
        .exchange_refresh_token("refresh-secret", 5_000)
        .await
        .unwrap_err();
    assert_eq!(failure.kind, TokenRefreshFailureKind::Transient);
    assert_eq!(failure.code, "refresh_token_reused");
    let rendered = format!("{failure:?}");
    assert!(!rendered.contains("refresh-secret"));
    assert!(!rendered.contains("provider-body-secret"));
    server.abort();
}

#[test]
fn oversized_jwt_payload_is_rejected() {
    let oversized = json!({ "padding": "x".repeat(16 * 1024 + 1) });
    let token = jwt(oversized);
    let error = parse_identity_claims(&token).unwrap_err();
    assert_eq!(error.code, OAuthErrorCode::InvalidJwt);
    assert!(!format!("{error:?}").contains(&"x".repeat(128)));
}

async fn exchange(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/x-www-form-urlencoded")));
    let fields: HashMap<_, _> = url::form_urlencoded::parse(&body).into_owned().collect();
    assert_eq!(
        fields.get("grant_type").map(String::as_str),
        Some("authorization_code")
    );
    assert_eq!(
        fields.get("code").map(String::as_str),
        Some("authorization-code")
    );
    assert_eq!(
        fields.get("redirect_uri").map(String::as_str),
        Some("http://localhost:1455/auth/callback")
    );
    assert_eq!(
        fields.get("client_id").map(String::as_str),
        Some(CODEX_OAUTH_CLIENT_ID)
    );
    assert!(fields
        .get("code_verifier")
        .is_some_and(|value| value.len() >= 43));
    Json(json!({
        "access_token": "access-token",
        "refresh_token": "refresh-token",
        "id_token": jwt(json!({
            "email": "user@example.test",
            "exp": 4_000,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_subscription_active_until": "2026-09-10T00:00:00Z",
                "chatgpt_user_id": "user-123",
                "chatgpt_account_id": "account-123"
            }
        })),
        "expires_in": 3_600
    }))
}

async fn refresh_without_rotation(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["client_id"], CODEX_OAUTH_CLIENT_ID);
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "refresh-secret");
    Json(json!({ "access_token": "new-access-token", "expires_in": 60 }))
}

async fn refresh_failure() -> impl IntoResponse {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "code": "refresh_token_reused",
                "message": "provider-body-secret"
            }
        })),
    )
}

fn test_client(base_url: &Url) -> CodexOAuthClient {
    CodexOAuthClient::with_endpoints(
        base_url.join("oauth/authorize").unwrap(),
        base_url.join("oauth/token").unwrap(),
    )
    .unwrap()
}

fn jwt(payload: Value) -> String {
    format!(
        "{}.{}.signature",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
    )
}

async fn spawn(router: Router) -> (Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (Url::parse(&format!("http://{address}/")).unwrap(), server)
}

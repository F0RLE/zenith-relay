use super::*;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

#[test]
fn discovery_failure_codes_keep_the_management_contract_stable() {
    let cases = [
        (
            ModelDiscoveryFailureCode::AgentTaskInvalid,
            "models_agent_task_invalid",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::Forbidden,
            "models_forbidden",
            false,
            true,
        ),
        (
            ModelDiscoveryFailureCode::HttpStatus,
            "models_http_status",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidAccessToken,
            "models_invalid_access_token",
            true,
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidAccountId,
            "models_invalid_account_id",
            true,
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidClientVersion,
            "models_invalid_client_version",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidEndpoint,
            "models_invalid_endpoint",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidResponse,
            "models_invalid_response",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::RateLimited,
            "models_rate_limited",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::ResponseTooLarge,
            "models_response_too_large",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::Transport,
            "models_transport",
            false,
            false,
        ),
        (
            ModelDiscoveryFailureCode::Unauthorized,
            "models_unauthorized",
            true,
            false,
        ),
        (
            ModelDiscoveryFailureCode::Upstream,
            "models_upstream",
            false,
            false,
        ),
    ];

    for (code, management_code, authentication_failure, blocks_account) in cases {
        assert_eq!(code.management_code(), management_code);
        assert_eq!(code.is_authentication_failure(), authentication_failure);
        assert_eq!(code.blocks_account(), blocks_account);
    }
}

#[tokio::test]
async fn discovery_retains_provider_retry_after_for_the_shared_scheduler() {
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "120")],
                "{}",
            )
        }),
    ))
    .await;
    let failure = CodexModelsClient::with_endpoint(endpoint)
        .unwrap()
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await
        .unwrap_err();
    assert_eq!(failure.code, ModelDiscoveryFailureCode::RateLimited);
    assert_eq!(failure.retry_after_ms, Some(120_000));
    server.abort();
}

#[tokio::test]
async fn discovers_unique_account_slugs_with_codex_request_contract() {
    let (endpoint, server) =
        spawn(Router::new().route("/backend-api/codex/models", get(successful_models))).await;
    let models = CodexModelsClient::with_endpoint(endpoint)
        .unwrap()
        .discover(
            "access-secret",
            "account-123",
            super::super::CODEX_MODELS_CLIENT_VERSION,
        )
        .await
        .unwrap();

    assert_eq!(
        models,
        vec![
            "gpt-5",
            "gpt-hidden",
            "gpt-internal",
            "gpt-legacy",
            "gpt-5-mini"
        ]
    );
    let rendered = format!("{models:?}");
    assert!(!rendered.contains("description-secret"));
    assert!(!rendered.contains("instructions-secret"));
    server.abort();
}

#[tokio::test]
async fn discovers_models_from_a_catalog_larger_than_512_kib() {
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(|| async {
            Json(json!({
                "models": [{
                    "slug": "gpt-5",
                    "base_instructions": "x".repeat(600 * 1024)
                }]
            }))
        }),
    ))
    .await;
    let models = CodexModelsClient::with_endpoint(endpoint)
        .unwrap()
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await
        .unwrap();
    assert_eq!(models, vec!["gpt-5"]);
    server.abort();
}

#[tokio::test]
async fn malformed_oversized_and_http_errors_are_redacted() {
    for (handler, expected, retryable) in [
        (
            get(malformed_models),
            ModelDiscoveryFailureCode::InvalidResponse,
            false,
        ),
        (
            get(oversized_models),
            ModelDiscoveryFailureCode::ResponseTooLarge,
            false,
        ),
        (
            get(upstream_failure),
            ModelDiscoveryFailureCode::Upstream,
            true,
        ),
    ] {
        let (endpoint, server) =
            spawn(Router::new().route("/backend-api/codex/models", handler)).await;
        let error = CodexModelsClient::with_endpoint(endpoint)
            .unwrap()
            .discover(
                "access-secret",
                "account-123",
                super::super::CODEX_MODELS_CLIENT_VERSION,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, expected);
        assert_eq!(error.retryable, retryable);
        let rendered = format!("{error:?} {error}");
        for secret in [
            "access-secret",
            "account-123",
            "provider-body-secret",
            "description-secret",
            "instructions-secret",
        ] {
            assert!(!rendered.contains(secret));
        }
        server.abort();
    }
}

async fn successful_models(headers: HeaderMap, uri: Uri) -> impl IntoResponse {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer access-secret")
    );
    assert_eq!(
        headers
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok()),
        Some("account-123")
    );
    assert_eq!(
        headers
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some(super::super::CODEX_ORIGINATOR)
    );
    let expected_query = format!(
        "client_version={}",
        super::super::CODEX_MODELS_CLIENT_VERSION
    );
    assert_eq!(uri.query(), Some(expected_query.as_str()));
    assert_eq!(
        headers.get("version").and_then(|value| value.to_str().ok()),
        Some(super::super::CODEX_MODELS_CLIENT_VERSION)
    );
    Json(json!({
        "models": [
            {
                "slug": "gpt-5",
                "supported_in_api": true,
                "description": "description-secret",
                "base_instructions": "instructions-secret"
            },
            { "slug": " gpt-5 " },
            { "slug": "gpt-hidden", "supported_in_api": false },
            { "slug": "gpt-internal", "visibility": "hide" },
            { "slug": "gpt-legacy", "visibility": "hide", "upgrade": { "model": "gpt-5" } },
            { "slug": "" },
            { "slug": "gpt-5-mini" }
        ]
    }))
}

async fn malformed_models() -> impl IntoResponse {
    (StatusCode::OK, "provider-body-secret")
}

async fn oversized_models() -> impl IntoResponse {
    (
        StatusCode::OK,
        format!(
            "provider-body-secret{}",
            "x".repeat(MAX_MODELS_RESPONSE_BYTES)
        ),
    )
}

async fn upstream_failure() -> impl IntoResponse {
    (StatusCode::BAD_GATEWAY, "provider-body-secret")
}

async fn spawn(router: Router) -> (Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (
        Url::parse(&format!("http://{address}/backend-api/codex/models")).unwrap(),
        server,
    )
}

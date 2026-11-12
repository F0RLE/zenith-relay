use super::*;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

mod recovery;

#[test]
fn siwc_catalog_distinguishes_no_visible_models_from_a_malformed_response() {
    for body in [
        br#"{"models":[]}"#.as_slice(),
        br#"{"models":[{"slug":"gpt-hidden","visibility":"hide"}]}"#.as_slice(),
    ] {
        assert!(parse::parse_siwc_models(body).unwrap().is_empty());
    }
    for body in [
        br#"{"data":[]}"#.as_slice(),
        br#"{"models":null}"#.as_slice(),
        br#"{"models":[{"slug":"","visibility":"list"}]}"#.as_slice(),
    ] {
        assert_eq!(
            parse::parse_siwc_models(body).unwrap_err().code,
            ModelDiscoveryFailureCode::InvalidResponse
        );
    }
}

#[tokio::test]
async fn siwc_catalog_uses_only_bearer_and_preserves_visible_server_order() {
    let router = Router::new().route("/v1/models", get(|headers: HeaderMap, uri: axum::http::Uri| async move {
        assert_eq!(headers.get("authorization").unwrap(), "Bearer synthetic-token");
        assert!(!headers.contains_key("chatgpt-account-id"));
        assert!(!headers.contains_key("originator"));
        assert!(uri.query().is_none());
        Json(json!({"models":[{"slug":"z-model","display_name":"Z model","visibility":"list"},{"slug":"hidden","visibility":"hide"},{"slug":"a-model","visibility":"list"}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!(
        "http://{}/v1/models",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let models = AccountModelsClient::with_endpoint(endpoint)
        .unwrap()
        .with_oauth_client_kind(super::super::OAuthClientKind::Siwc)
        .discover("synthetic-token", "synthetic-registration", "ignored")
        .await
        .unwrap();
    assert_eq!(models, vec!["z-model", "a-model"]);
    task.abort();
}

#[test]
fn discovery_failure_codes_keep_the_management_contract_stable() {
    let cases = [
        (
            ModelDiscoveryFailureCode::AgentTaskInvalid,
            "models_agent_task_invalid",
            false,
        ),
        (
            ModelDiscoveryFailureCode::Forbidden,
            "models_forbidden",
            false,
        ),
        (
            ModelDiscoveryFailureCode::HttpStatus,
            "models_http_status",
            false,
        ),
        (
            ModelDiscoveryFailureCode::EndpointNotFound,
            "models_endpoint_not_found",
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidAccessToken,
            "models_invalid_access_token",
            true,
        ),
        (
            ModelDiscoveryFailureCode::InvalidAccountId,
            "models_invalid_account_id",
            true,
        ),
        (
            ModelDiscoveryFailureCode::InvalidClientVersion,
            "models_invalid_client_version",
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidEndpoint,
            "models_invalid_endpoint",
            false,
        ),
        (
            ModelDiscoveryFailureCode::InvalidResponse,
            "models_invalid_response",
            false,
        ),
        (
            ModelDiscoveryFailureCode::RateLimited,
            "models_rate_limited",
            false,
        ),
        (
            ModelDiscoveryFailureCode::ResponseTooLarge,
            "models_response_too_large",
            false,
        ),
        (
            ModelDiscoveryFailureCode::Transport,
            "models_transport",
            false,
        ),
        (
            ModelDiscoveryFailureCode::Unauthorized,
            "models_unauthorized",
            true,
        ),
        (
            ModelDiscoveryFailureCode::Upstream,
            "models_upstream",
            false,
        ),
    ];

    for (code, management_code, authentication_failure) in cases {
        assert_eq!(code.management_code(), management_code);
        assert_eq!(code.is_authentication_failure(), authentication_failure);
    }
}

#[tokio::test]
async fn model_catalog_not_found_keeps_http_evidence_without_retry_or_auth_failure() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    for kind in [
        super::super::OAuthClientKind::Codex,
        super::super::OAuthClientKind::ExcelBps,
        super::super::OAuthClientKind::Siwc,
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let (endpoint, server) = spawn(Router::new().route(
            "/backend-api/codex/models",
            get({
                let requests = requests.clone();
                move || {
                    requests.fetch_add(1, Ordering::SeqCst);
                    async { (StatusCode::NOT_FOUND, "synthetic-private-provider-body") }
                }
            }),
        ))
        .await;
        let failure = AccountModelsClient::with_endpoint(endpoint)
            .unwrap()
            .with_oauth_client_kind(kind)
            .discover(
                "synthetic-access",
                "synthetic-account",
                super::super::CODEX_MODELS_CLIENT_VERSION,
            )
            .await
            .unwrap_err();
        assert_eq!(failure.code, ModelDiscoveryFailureCode::EndpointNotFound);
        assert_eq!(failure.http_status, Some(404));
        assert_eq!(failure.code.management_code(), "models_endpoint_not_found");
        assert!(!failure.retryable);
        assert!(!failure.code.is_authentication_failure());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let diagnostic = format!("{failure:?} {failure}");
        assert!(!diagnostic.contains("synthetic-private-provider-body"));
        assert!(!diagnostic.contains("synthetic-access"));
        server.abort();
    }
}

#[tokio::test]
async fn bps_free_token_claims_do_not_replace_provider_access_checks() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let token = format!(
        "e30.{}.synthetic",
        URL_SAFE_NO_PAD.encode(
            json!({
                "https://api.openai.com/auth": {"chatgpt_plan_type":"chatgpt_free_tier"}
            })
            .to_string()
        )
    );
    for status in [StatusCode::OK, StatusCode::FORBIDDEN] {
        let (mut endpoint, server) = spawn(Router::new().route(
            "/basispoints/api/responses/access",
            get(move || async move {
                (
                    status,
                    Json(
                        json!({"allowed": true, "model_catalog": {"models": [{"id": "gpt-bps"}]}}),
                    ),
                )
            }),
        ))
        .await;
        endpoint.set_path("/basispoints/api/responses/access");
        let client = AccountModelsClient::with_endpoint(endpoint)
            .unwrap()
            .with_oauth_client_kind(super::super::OAuthClientKind::ExcelBps);
        let result = client
            .discover(&token, "synthetic-account", "ignored")
            .await;
        if status == StatusCode::OK {
            assert_eq!(result.unwrap(), vec!["gpt-bps"]);
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code, ModelDiscoveryFailureCode::Forbidden);
            assert!(!error.retryable);
            assert_eq!(error.http_status, Some(403));
        }
        server.abort();
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
    let failure = AccountModelsClient::with_endpoint(endpoint)
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
    let models = AccountModelsClient::with_endpoint(endpoint)
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
    let models = AccountModelsClient::with_endpoint(endpoint)
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
        let error = AccountModelsClient::with_endpoint(endpoint)
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

#[tokio::test]
async fn bps_discovery_keeps_custom_fixture_endpoint_and_uses_access_contract() {
    let (mut endpoint, server) = spawn(Router::new().route(
        "/basispoints/api/responses/access",
        get(successful_basis_points_access),
    ))
    .await;
    endpoint.set_path("/basispoints/api/responses/access");
    let models = AccountModelsClient::with_endpoint(endpoint)
        .unwrap()
        .with_oauth_client_kind(super::super::OAuthClientKind::ExcelBps)
        .discover("access-secret", "account-123", "ignored")
        .await
        .unwrap();
    assert_eq!(models, vec!["gpt-bps"]);
    server.abort();
}

#[tokio::test]
async fn bps_discovery_returns_catalog_decisions_from_the_same_access_response() {
    let (mut endpoint, server) = spawn(Router::new().route(
        "/basispoints/api/responses/access",
        get(|| async {
            Json(json!({
                "allowed":true,
                "model_catalog":{"models":[
                    {"id":"gpt-synthetic-new"},
                    {"id":"gpt-synthetic-picker", "model_picker_enabled":false}
                ]}
            }))
        }),
    ))
    .await;
    endpoint.set_path("/basispoints/api/responses/access");
    let discovery = AccountModelsClient::with_endpoint(endpoint)
        .unwrap()
        .with_oauth_client_kind(super::super::OAuthClientKind::ExcelBps)
        .discover_authorized_with_diagnostics(
            reqwest::header::HeaderValue::from_static("Bearer synthetic-access"),
            "synthetic-account",
            "ignored",
        )
        .await
        .unwrap();
    assert_eq!(discovery.models, ["gpt-synthetic-new"]);
    let diagnostics = discovery.basis_points_diagnostics.unwrap();
    assert_eq!(diagnostics.received_models, 2);
    assert_eq!(diagnostics.accepted_models, 1);
    assert_eq!(diagnostics.models[1].outcome.as_str(), "picker_disabled");
    assert!(!format!("{diagnostics:?}").contains("synthetic-access"));
    server.abort();
}

async fn successful_basis_points_access(headers: HeaderMap, uri: Uri) -> impl IntoResponse {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer access-secret")
    );
    assert_eq!(
        headers
            .get("x-basispoints-auth-mode")
            .and_then(|value| value.to_str().ok()),
        Some("chatgpt")
    );
    assert_eq!(
        headers
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok()),
        Some("account-123")
    );
    assert_eq!(uri.query(), Some("include_models=true"));
    Json(json!({
        "allowed": true,
        "model_catalog": {
            "models": [{"id": "gpt-bps"}]
        }
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

#[test]
fn codex_discovery_preserves_provider_order_independently_of_default_priorities() {
    let models = super::parse::parse_models(
        br#"{"models":[
        {"slug":"synthetic-zeta","priority":10,"is_default":false},
        {"slug":"synthetic-alpha","priority":0,"is_default":true},
        {"slug":"synthetic-zeta","priority":-1},
        {"slug":"synthetic-middle","priority":1}
    ]}"#,
    )
    .unwrap();
    assert_eq!(
        models,
        ["synthetic-zeta", "synthetic-alpha", "synthetic-middle"]
    );
}

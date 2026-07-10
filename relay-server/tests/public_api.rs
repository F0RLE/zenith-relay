use axum::{
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{net::SocketAddr, path::Path, sync::Arc};
use tempfile::TempDir;
use zenith_relay_server::{
    config::Config,
    http,
    state::AppState,
    store::{Store, Vault},
};

#[tokio::test]
async fn remote_gateway_persists_and_serves_after_management_client_disconnects() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_upstream().await;
    let first = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    assert_eq!(
        client
            .get(format!("{}/state", first.origin))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let source = client
        .post(format!("{}/sources", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Synthetic upstream",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(source.status(), StatusCode::CREATED);

    let generated: Value = client
        .post(format!("{}/keys", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Test client",
            "sourceIds": null,
            "accountIds": null,
            "allowedModels": [],
            "excludedModels": [],
            "modelPrefix": null
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pool_key = generated["secret"].as_str().unwrap().to_string();

    let models = client
        .get(format!("{}/v1/models", first.origin))
        .bearer_auth(&pool_key)
        .send()
        .await
        .unwrap();
    assert_eq!(models.status(), StatusCode::OK);
    assert!(models.text().await.unwrap().contains("gpt-test"));

    let response = client
        .post(format!("{}/v1/responses", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","input":"synthetic request"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("response-test"));

    let preview: Value = client
        .post(format!("{}/accounts/import/preview", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Synthetic OAuth account",
            "accessToken": "synthetic-access-token",
            "refreshToken": "synthetic-refresh-token",
            "expiresAtMs": 4_000_000_000_000_u64,
            "chatgptAccountId": "synthetic-chatgpt-account-id",
            "responsesUrl": format!("{upstream}/account/responses"),
            "models": ["gpt-test"],
            "priority": 10,
            "weight": 1
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let preview_text = preview.to_string();
    assert!(!preview_text.contains("synthetic-access-token"));
    assert!(!preview_text.contains("synthetic-refresh-token"));
    let session_id = preview["sessionId"].as_str().unwrap();
    let confirmed = client
        .post(format!("{}/accounts/import/confirm", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": session_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed_text = confirmed.text().await.unwrap();
    assert!(!confirmed_text.contains("synthetic-access-token"));

    let account_response = client
        .post(format!("{}/v1/responses", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","input":"synthetic account request"}))
        .send()
        .await
        .unwrap();
    assert_eq!(account_response.status(), StatusCode::OK);
    assert!(account_response
        .text()
        .await
        .unwrap()
        .contains("account-response-test"));

    let state_text = client
        .get(format!("{}/state", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!state_text.contains("synthetic-upstream-api-key"));
    assert!(!state_text.contains("synthetic-access-token"));
    assert!(!state_text.contains("synthetic-refresh-token"));
    assert!(!state_text.contains(&pool_key));

    let first_server_id: String = serde_json::from_str::<Value>(&state_text).unwrap()
        ["runtimeTarget"]["serverId"]
        .as_str()
        .unwrap()
        .to_string();
    first.task.abort();
    let _ = first.task.await;
    drop(first.state);

    let second = spawn_server(root.path()).await;
    let second_state: Value = client
        .get(format!("{}/state", second.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        second_state["runtimeTarget"]["serverId"].as_str(),
        Some(first_server_id.as_str())
    );
    assert_eq!(
        client
            .get(format!("{}/v1/models", second.origin))
            .bearer_auth(&pool_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let database = std::fs::read(root.path().join("relay.sqlite")).unwrap();
    assert!(!String::from_utf8_lossy(&database).contains("synthetic-upstream-api-key"));
    assert!(!String::from_utf8_lossy(&database).contains("synthetic-access-token"));
    let vault = std::fs::read(root.path().join("vault").join("secrets.enc")).unwrap();
    assert!(!String::from_utf8_lossy(&vault).contains("synthetic-upstream-api-key"));
    assert!(!String::from_utf8_lossy(&vault).contains("synthetic-access-token"));
    assert!(!String::from_utf8_lossy(&vault).contains(&pool_key));

    second.task.abort();
    upstream_task.abort();
}

struct RunningServer {
    origin: String,
    state: Arc<AppState>,
    task: tokio::task::JoinHandle<()>,
}

async fn spawn_server(root: &Path) -> RunningServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = Config {
        bind: address,
        data_dir: root.to_path_buf(),
        public_base_url: url::Url::parse(&format!("http://{address}")).unwrap(),
        management_token: "synthetic-management-token-value".to_string(),
        vault_key: [9; 32],
    };
    let store = Arc::new(Store::open(root.join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.join("vault"), config.vault_key).unwrap());
    let state = AppState::new(config, store, vault).unwrap();
    state.rebuild_runtime().await.unwrap();
    let router = http::router(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    RunningServer {
        origin: format!("http://{address}"),
        state,
        task,
    }
}

async fn spawn_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data":[{"id":"gpt-test"}]})) }),
        )
        .route("/v1/responses", post(upstream_response))
        .route("/account/responses", post(account_response));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn upstream_response(request: Request) -> impl IntoResponse {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    (
        StatusCode::OK,
        Json(json!({
            "id":"response-test",
            "object":"response",
            "model":"gpt-test",
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        })),
    )
}

async fn account_response(request: Request) -> impl IntoResponse {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-access-token")
    );
    assert_eq!(
        request
            .headers()
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok()),
        Some("synthetic-chatgpt-account-id")
    );
    (
        StatusCode::OK,
        Json(json!({
            "id":"account-response-test",
            "object":"response",
            "model":"gpt-test",
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        })),
    )
}

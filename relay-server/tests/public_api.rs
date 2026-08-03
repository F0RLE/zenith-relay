use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST},
        StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    net::SocketAddr,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zenith_relay_server::{
    config::Config,
    http,
    state::AppState,
    store::{Store, Vault},
};

#[tokio::test]
async fn profile_can_be_prepared_before_the_first_account_transfer() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let credential = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();

    assert_eq!(credential.status(), StatusCode::OK);
    let credential: Value = credential.json().await.unwrap();
    assert_eq!(credential["keyId"], "key_system");
    assert_eq!(credential["baseUrl"], format!("{}/v1", server.origin));
    assert!(credential["secret"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
}

#[tokio::test]
async fn source_creation_persists_only_models_confirmed_by_each_protocol() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_mixed_protocol_upstream().await;
    let server = spawn_server(root.path()).await;
    let source = reqwest::Client::new()
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Mixed native source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "protocolBindings": [
                {"wireApi": "responses", "modelIds": []},
                {"wireApi": "messages", "modelIds": []}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(source.status(), StatusCode::CREATED);
    let source: Value = source.json().await.unwrap();
    assert_eq!(source["wireApi"], "responses");
    assert_eq!(source["models"], json!(["gpt-native", "claude-native"]));
    assert_eq!(
        source["protocolBindings"],
        json!([
            {
                "wireApi": "responses",
                "adapter": "native",
                "reasoningMode": "disabled",
                "modelIds": ["gpt-native"]
            },
            {
                "wireApi": "messages",
                "adapter": "native",
                "reasoningMode": "disabled",
                "modelIds": ["claude-native"]
            }
        ])
    );

    server.task.abort();
    upstream_task.abort();
}

#[tokio::test]
async fn source_creation_preserves_native_and_bridged_responses_routes() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_mixed_protocol_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Mixed Responses source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "protocolBindings": [
                {
                    "wireApi": "responses",
                    "adapter": "native",
                    "reasoningMode": "disabled",
                    "modelIds": []
                },
                {
                    "wireApi": "responses",
                    "adapter": "responses_to_messages",
                    "reasoningMode": "adaptive",
                    "modelIds": []
                }
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let source_id = source["id"].as_str().unwrap();

    assert_eq!(source["wireApi"], "responses");
    assert_eq!(source["models"], json!(["gpt-native", "claude-native"]));
    assert_eq!(
        source["protocolBindings"],
        json!([
            {
                "wireApi": "responses",
                "adapter": "native",
                "reasoningMode": "disabled",
                "modelIds": ["gpt-native"]
            },
            {
                "wireApi": "responses",
                "adapter": "responses_to_messages",
                "reasoningMode": "adaptive",
                "modelIds": ["claude-native"]
            }
        ])
    );

    let membership = client
        .post(format!("{}/pool/members", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sourceIds": [source_id], "inPool": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);

    let stored = server.state.store.sources().unwrap();
    assert!(stored[0]
        .supports_wire_api(zenith_relay_core::WireApi::Responses)
        .unwrap());
    assert_eq!(
        serde_json::to_value(&stored[0].protocol_bindings).unwrap(),
        source["protocolBindings"]
    );

    server.task.abort();
    upstream_task.abort();
}

#[tokio::test]
async fn scoped_native_messages_source_stays_outside_the_responses_system_pool() {
    let root = TempDir::new().unwrap();
    let (responses_upstream, responses_task) = spawn_upstream().await;
    let (messages_upstream, messages_state, messages_task) = spawn_messages_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let responses_source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Responses source",
            "baseUrl": format!("{responses_upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let responses_source_id = responses_source["id"].as_str().unwrap();
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({
                "sourceIds": [responses_source_id],
                "inPool": true
            }))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let messages_source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Native Messages source",
            "baseUrl": format!("{messages_upstream}/v1"),
            "apiKey": "messages-source-key",
            "wireApi": "messages",
            "protocolBindings": [{
                "wireApi": "messages",
                "modelIds": ["claude-native"]
            }],
            "models": ["claude-native"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let messages_source_id = messages_source["id"].as_str().unwrap();

    let pool_error = client
        .post(format!("{}/pool/members", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sourceIds": [messages_source_id],
            "inPool": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(pool_error.status(), StatusCode::CONFLICT);
    assert_eq!(
        pool_error.json::<Value>().await.unwrap()["error"]["code"],
        "source_pool_protocol_unsupported"
    );

    let messages_key: Value = client
        .post(format!("{}/keys", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "schemaVersion": 1,
            "label": "Native Messages key",
            "sourceIds": [messages_source_id],
            "accountIds": [],
            "allowedModels": [],
            "excludedModels": [],
            "modelPrefix": null,
            "wireApis": ["messages"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let messages_secret = messages_key["secret"].as_str().unwrap();
    assert_eq!(messages_key["key"]["wireApis"], json!(["messages"]));

    let started = client
        .post(format!("{}/gateway/start", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);

    let system_credential: Value = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let system_secret = system_credential["secret"].as_str().unwrap();
    let system_models: Value = client
        .get(format!("{}/v1/models", server.origin))
        .bearer_auth(system_secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let system_model_ids = system_models["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<Vec<_>>();
    assert!(system_model_ids.contains(&"gpt-test"));
    assert!(!system_model_ids.contains(&"claude-native"));

    let request = json!({
        "model": "claude-native",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "use the tool"}],
        "tools": [{
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }
        }],
        "tool_choice": {"type": "auto"}
    });
    let messages_response = client
        .post(format!("{}/v1/messages", server.origin))
        .header("x-api-key", messages_secret)
        .header("anthropic-beta", "fine-grained-tool-streaming-2025-05-14")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(messages_response.status(), StatusCode::OK);
    assert_eq!(
        messages_response.json::<Value>().await.unwrap()["type"],
        "message"
    );

    let denied = client
        .post(format!("{}/v1/messages", server.origin))
        .header("x-api-key", system_secret)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied: Value = denied.json().await.unwrap();
    assert_eq!(denied["type"], "error");
    assert_eq!(denied["error"]["type"], "permission_error");

    let denied = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(messages_secret)
        .json(&json!({"model": "claude-native", "input": "wrong endpoint"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let observed = messages_state.lock().unwrap().clone();
    assert_eq!(observed.len(), 2);
    assert_eq!(
        observed[0].x_api_key.as_deref(),
        Some("messages-source-key")
    );
    assert_eq!(observed[0].anthropic_version.as_deref(), Some("2023-06-01"));
    assert_eq!(
        observed[1].anthropic_beta.as_deref(),
        Some("fine-grained-tool-streaming-2025-05-14")
    );
    assert_eq!(observed[1].body, request);

    server.task.abort();
    responses_task.abort();
    messages_task.abort();
}

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

    let source_response = client
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
    let source_status = source_response.status();
    let source_text = source_response.text().await.unwrap();
    assert_eq!(source_status, StatusCode::CREATED, "{source_text}");
    assert!(!source_text.contains("synthetic-upstream-api-key"));
    let source: Value = serde_json::from_str(&source_text).unwrap();
    let source_id = source["id"].as_str().unwrap();
    let tested_source: Value = client
        .post(format!("{}/sources/{source_id}/test", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tested_source["models"], json!(["gpt-test"]));
    assert!(!tested_source
        .to_string()
        .contains("synthetic-upstream-api-key"));
    let membership = client
        .post(format!("{}/pool/members", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sourceIds": [source_id], "inPool": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);

    let started: Value = client
        .post(format!("{}/gateway/start", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(started["gateway"]["running"], true);

    let capabilities: Value = client
        .get(format!("{}/capabilities", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(capabilities["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feature| feature == "profile_attach"));
    assert!(capabilities["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feature| feature == "profile_key_rotation"));
    let profile_response = client
        .get(format!("{}/profile/credential", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(
        profile_response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let profile_credential: Value = profile_response.json().await.unwrap();
    let mut profile_key = profile_credential["secret"].as_str().unwrap().to_string();
    assert_eq!(profile_credential["keyId"], "key_system");
    assert_eq!(
        profile_credential["baseUrl"],
        format!("{}/v1", first.origin)
    );
    let keys: Value = client
        .get(format!("{}/keys", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(keys["schemaVersion"], 1);
    assert!(keys["keys"].as_array().unwrap().is_empty());
    assert_eq!(
        client
            .delete(format!("{}/keys/key_system", first.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        client
            .get(format!("{}/v1/models", first.origin))
            .bearer_auth(&profile_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let aborted_rotation: Value = client
        .post(format!("{}/profile/credential/rotations", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let aborted_rotation_id = aborted_rotation["rotationId"].as_str().unwrap();
    let aborted_secret = aborted_rotation["secret"].as_str().unwrap();
    assert_eq!(aborted_rotation["schemaVersion"], 1);
    assert_eq!(aborted_rotation["keyId"], "key_system");
    assert_eq!(
        client
            .get(format!("{}/v1/models", first.origin))
            .bearer_auth(aborted_secret)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .delete(format!(
                "{}/profile/credential/rotations/{aborted_rotation_id}",
                first.origin
            ))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        client
            .get(format!("{}/v1/models", first.origin))
            .bearer_auth(aborted_secret)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let rotation_response = client
        .post(format!("{}/profile/credential/rotations", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(
        rotation_response.headers().get(CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let rotation: Value = rotation_response.json().await.unwrap();
    let rotation_id = rotation["rotationId"].as_str().unwrap();
    let rotated_profile_key = rotation["secret"].as_str().unwrap().to_string();
    assert_ne!(rotated_profile_key, profile_key);
    assert_eq!(
        client
            .get(format!("{}/v1/models", first.origin))
            .bearer_auth(&rotated_profile_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!(
                "{}/profile/credential/rotations/{rotation_id}",
                first.origin
            ))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        client
            .get(format!("{}/v1/models", first.origin))
            .bearer_auth(&profile_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    profile_key = rotated_profile_key;
    let hidden_keys: Value = client
        .get(format!("{}/keys", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(hidden_keys["keys"].as_array().unwrap().is_empty());

    let generated: Value = client
        .post(format!("{}/keys", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "schemaVersion": 1,
            "label": "Test client",
            "sourceIds": null,
            "accountIds": null,
            "allowedModels": [],
            "excludedModels": [],
            "modelPrefix": null,
            "wireApis": ["responses"],
            "softBudgetMicroUsd": 2
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(generated["schemaVersion"], 1);
    assert_eq!(generated["key"]["wireApis"], json!(["responses"]));
    assert_eq!(generated["key"]["softBudgetMicroUsd"], 2);
    let client_key_id = generated["key"]["id"].as_str().unwrap().to_string();
    let mut pool_key = generated["secret"].as_str().unwrap().to_string();

    for path in ["/v1/chat/completions", "/v1/images/generations"] {
        let denied = client
            .post(format!("{}{path}", first.origin))
            .bearer_auth(&pool_key)
            .json(&json!({"model":"gpt-test"}))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            denied.json::<Value>().await.unwrap()["error"]["code"],
            "client_api_not_allowed"
        );
    }

    let models = client
        .get(format!("{}/v1/models", first.origin))
        .bearer_auth(&pool_key)
        .send()
        .await
        .unwrap();
    assert_eq!(models.status(), StatusCode::OK);
    assert!(models.text().await.unwrap().contains("gpt-test"));
    assert_websocket_upgrade(&first.origin, &pool_key).await;

    let chat_only: Value = client
        .post(format!("{}/keys", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "schemaVersion": 1,
            "label": "Chat-only client",
            "sourceIds": null,
            "accountIds": null,
            "allowedModels": [],
            "excludedModels": [],
            "modelPrefix": null,
            "wireApis": ["chat_completions"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let chat_only_key = chat_only["secret"].as_str().unwrap();
    for path in ["/v1/responses", "/v1/responses/compact", "/v1/alpha/search"] {
        assert_eq!(
            client
                .post(format!("{}{path}", first.origin))
                .bearer_auth(chat_only_key)
                .json(&json!({"model":"gpt-test"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_websocket_status(&first.origin, chat_only_key, "403").await;
    assert_eq!(
        client
            .delete(format!(
                "{}/keys/{}",
                first.origin,
                chat_only["key"]["id"].as_str().unwrap()
            ))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    let response = client
        .post(format!("{}/v1/responses", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","input":"synthetic request"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("response-test"));

    let response = client
        .post(format!("{}/v1/responses", first.origin))
        .header(HOST, "relay.example.test")
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","input":"external host"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("response-test"));

    let streamed = client
        .post(format!("{}/v1/responses", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","input":"synthetic stream","stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(streamed.status(), StatusCode::OK);
    assert_eq!(
        streamed
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert!(streamed
        .text()
        .await
        .unwrap()
        .contains("response.completed"));

    let non_stream_diagnostic: Value = client
        .post(format!("{}/diagnostics", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"stream": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(non_stream_diagnostic["stream"], false);
    assert_eq!(non_stream_diagnostic["model"], "gpt-test");

    let diagnostic: Value = client
        .post(format!("{}/diagnostics", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"stream": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(diagnostic["stream"], true);
    assert_eq!(diagnostic["model"], "gpt-test");

    let deadline = Instant::now() + Duration::from_secs(5);
    let usage = loop {
        let usage: Value = client
            .get(format!(
                "{}/usage?page=1&pageSize=1&range=daily&modelQuery=gpt-test&success=true",
                first.origin
            ))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if usage["total"].as_u64().is_some_and(|total| total >= 2) {
            break usage;
        }
        assert!(Instant::now() < deadline, "usage queue did not drain");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(usage["total"].as_u64().is_some_and(|total| total >= 2));
    assert_eq!(usage["events"].as_array().unwrap().len(), 1);
    assert!(usage["totalPages"].as_u64().is_some_and(|pages| pages >= 2));

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
    let confirmed_json: Value = serde_json::from_str(&confirmed_text).unwrap();
    let account_id = confirmed_json["id"].as_str().unwrap();
    let updated: Value = client
        .patch(format!("{}/accounts/{account_id}", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"enabled": false, "draining": true, "priority": 25, "weight": 2}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["draining"], true);
    let reenabling = client
        .patch(format!("{}/accounts/{account_id}", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"enabled": true, "draining": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(reenabling.status(), StatusCode::OK);
    let membership = client
        .post(format!("{}/pool/members", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"accountIds": [account_id], "inPool": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);

    assert_eq!(
        client
            .post(format!(
                "{}/accounts/{account_id}/identity/reveal",
                first.origin
            ))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let revealed = client
        .post(format!(
            "{}/accounts/{account_id}/identity/reveal",
            first.origin
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(revealed.status(), StatusCode::OK);
    assert_eq!(
        revealed
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store, max-age=0")
    );
    let revealed_text = revealed.text().await.unwrap();
    assert!(!revealed_text.contains("synthetic-access-token"));
    let revealed_json: Value = serde_json::from_str(&revealed_text).unwrap();
    assert_eq!(revealed_json["accountId"], account_id);
    assert_eq!(revealed_json["identity"], "synthetic-chatgpt-account-id");

    assert_eq!(
        client
            .post(format!("{}/accounts/export", first.origin))
            .json(&json!({"accountIds": [account_id], "format": "codex"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    for format in [
        "zenith",
        "cpa",
        "sub2api",
        "cockpit",
        "9router",
        "codex",
        "axon_hub",
        "codex_manager",
    ] {
        let exported = client
            .post(format!("{}/accounts/export", first.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({"accountIds": [account_id], "format": format}))
            .send()
            .await
            .unwrap();
        assert_eq!(exported.status(), StatusCode::OK, "{format}");
        assert_eq!(
            exported
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("no-store, max-age=0")
        );
        let document: Value = exported.json().await.unwrap();
        assert_eq!(document["accountCount"], 1);
        let content = document["content"].as_str().unwrap();
        assert!(content.contains("synthetic-access-token"), "{format}");
        assert!(content.contains("synthetic-refresh-token"), "{format}");
        assert!(!content.contains("proxy.example"), "{format}");
        serde_json::from_str::<Value>(content).unwrap();
    }
    let zenith_export: Value = client
        .post(format!("{}/accounts/export", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "accountIds": [account_id],
            "format": "zenith",
            "description": "Seller description"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let zenith_content: Value =
        serde_json::from_str(zenith_export["content"].as_str().unwrap()).unwrap();
    assert_eq!(zenith_content["format"], "zenith");
    assert_eq!(zenith_content["description"], "Seller description");
    assert_eq!(zenith_content["accounts"][0]["auth"]["type"], "oauth");
    assert_eq!(
        client
            .post(format!("{}/accounts/export", first.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({"accountIds": [account_id, account_id], "format": "codex"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let wake_task: Value = client
        .post(format!("{}/wake-tasks", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "id": "",
            "name": "Synthetic selected wake",
            "enabled": true,
            "accountSelector": {"kind": "account_ids", "values": [account_id]},
            "windowKinds": ["primary"],
            "modelPolicy": {"kind": "explicit", "value": "gpt-test"},
            "trigger": {"kind": "quota_full"},
            "fallbackSchedule": null,
            "executionPolicy": "automatic",
            "jitterSeconds": 0,
            "maxAttemptsPerCycle": 1,
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let wake_id = wake_task["id"].as_str().unwrap();
    let wake_test: Value = client
        .post(format!("{}/wake-tasks/{wake_id}/test", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(wake_test["taskId"], wake_id);
    assert_eq!(wake_test["status"], "ready");
    assert_eq!(wake_test["eligibleAccounts"], 1);

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

    let compact: Value = client
        .post(format!("{}/v1/responses/compact", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","input":"compact","stream":false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(compact["type"], "compaction");

    let search: Value = client
        .post(format!("{}/v1/alpha/search", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","id":"remote-session","query":"search"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(search["results"][0]["title"], "remote result");

    for path in [
        "/v1/chat/completions/v1/responses",
        "/v1/chat/completions/v1/responses/compact",
        "/backend-api/codex/alpha/search",
    ] {
        let response = client
            .post(format!("{}{path}", first.origin))
            .bearer_auth(&pool_key)
            .json(
                &json!({"model":"gpt-test","id":"remote-session","input":"alias","query":"alias"}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    let account_event = loop {
        let account_usage: Value = client
            .get(format!("{}/usage?page=1&pageSize=50", first.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(event) = account_usage["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["candidateKind"] == "account" && event["inputTokens"] == 1)
        {
            break event.clone();
        }
        assert!(Instant::now() < deadline, "account usage was not persisted");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(account_event["candidateLabel"], "Synthetic OAuth account");
    assert_eq!(account_event["inputTokens"], 1);
    assert_eq!(account_event["cachedInputTokens"], 1);
    assert_eq!(account_event["outputTokens"], 1);
    assert_eq!(account_event["totalTokens"], 2);
    let priced: Value = client
        .post(format!("{}/models/prices", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "modelId": "gpt-test",
            "inputMicroUsdPerMillion": 1_000_000,
            "cachedInputMicroUsdPerMillion": 1_000_000,
            "outputMicroUsdPerMillion": 2_000_000
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let priced_model = priced["gateway"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "gpt-test")
        .unwrap();
    assert_eq!(priced_model["customPrice"], true);
    assert_eq!(priced_model["inputMicroUsdPerMillion"], 1_000_000);
    assert_eq!(priced_model["cachedInputMicroUsdPerMillion"], 1_000_000);
    assert_eq!(priced_model["outputMicroUsdPerMillion"], 2_000_000);
    let repriced_usage: Value = client
        .get(format!(
            "{}/usage?requestIdQuery={}",
            first.origin,
            account_event["requestId"].as_str().unwrap()
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repriced_usage["totals"]["apiEquivalent"]["microUsd"], 3);
    assert_eq!(repriced_usage["totals"]["apiEquivalent"]["pricedTokens"], 2);
    assert_eq!(
        repriced_usage["totals"]["apiEquivalent"]["unpricedTokens"],
        0
    );
    let client_keys: Value = client
        .get(format!("{}/keys", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let client_key = client_keys["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|key| key["id"] == client_key_id)
        .unwrap();
    assert!(client_key["usageTotals"]["requests"].as_u64().unwrap() > 0);
    assert!(
        client_key["usageTotals"]["apiEquivalent"]["microUsd"]
            .as_u64()
            .unwrap()
            >= 3
    );
    let rotated_client_key: Value = client
        .post(format!("{}/keys/{client_key_id}/rotate", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let replacement = rotated_client_key["secret"].as_str().unwrap().to_string();
    assert_ne!(replacement, pool_key);
    assert_eq!(
        rotated_client_key["key"]["usageTotals"],
        client_key["usageTotals"]
    );
    assert_eq!(
        client
            .get(format!("{}/v1/models", first.origin))
            .bearer_auth(&pool_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    pool_key = replacement;

    let disabled_response = client
        .post(format!("{}/models/rules", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"modelId": "gpt-test", "enabled": false}))
        .send()
        .await
        .unwrap();
    let disabled_status = disabled_response.status();
    let disabled_text = disabled_response.text().await.unwrap();
    assert_eq!(disabled_status, StatusCode::OK, "{disabled_text}");
    let disabled: Value = serde_json::from_str(&disabled_text).unwrap();
    assert_eq!(disabled["gateway"]["models"][0]["enabled"], false);
    let hidden_models: Value = client
        .get(format!("{}/v1/models", first.origin))
        .bearer_auth(&pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hidden_models["data"], json!([]));

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
    assert!(!state_text.contains(&profile_key));

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
    let persisted_price = second_state["gateway"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "gpt-test")
        .unwrap();
    assert_eq!(persisted_price["customPrice"], true);
    assert_eq!(persisted_price["cachedInputMicroUsdPerMillion"], 1_000_000);
    let persisted_models: Value = client
        .get(format!("{}/v1/models", second.origin))
        .bearer_auth(&pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(persisted_models["data"], json!([]));
    let enabled_response = client
        .post(format!("{}/models/rules", second.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"modelId": "gpt-test", "enabled": true}))
        .send()
        .await
        .unwrap();
    let enabled_status = enabled_response.status();
    let enabled_text = enabled_response.text().await.unwrap();
    assert_eq!(enabled_status, StatusCode::OK, "{enabled_text}");
    let enabled: Value = serde_json::from_str(&enabled_text).unwrap();
    assert_eq!(enabled["gateway"]["models"][0]["enabled"], true);
    let reopened_profile_credential: Value = client
        .get(format!("{}/profile/credential", second.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reopened_profile_credential["secret"], profile_key);
    let restored_models: Value = client
        .get(format!("{}/v1/models", second.origin))
        .bearer_auth(&pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restored_models["data"][0]["id"], "gpt-test");
    let usage_before_stream: Value = client
        .get(format!("{}/usage?page=1&pageSize=1", second.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let total_before_stream = usage_before_stream["total"].as_u64().unwrap();
    let reopened_stream = client
        .post(format!("{}/v1/responses", second.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model":"gpt-test","input":"after desktop reopen","stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(reopened_stream.status(), StatusCode::OK);
    assert!(reopened_stream
        .text()
        .await
        .unwrap()
        .contains("response.completed"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reopened_usage: Value = client
            .get(format!(
                "{}/usage?page=1&pageSize=50&success=true",
                second.origin
            ))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if reopened_usage["total"]
            .as_u64()
            .is_some_and(|total| total > total_before_stream)
        {
            break;
        }
        assert!(Instant::now() < deadline, "stream usage was not persisted");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let usage_before_key_delete: Value = client
        .get(format!(
            "{}/usage?localKeyQuery={client_key_id}",
            second.origin
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(usage_before_key_delete["total"].as_u64().unwrap() > 0);
    assert_eq!(
        client
            .delete(format!("{}/keys/{client_key_id}", second.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let usage_after_key_delete: Value = client
        .get(format!(
            "{}/usage?localKeyQuery={client_key_id}",
            second.origin
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        usage_after_key_delete["totals"],
        usage_before_key_delete["totals"]
    );
    assert_eq!(
        client
            .delete(format!("{}/usage", second.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let cleared: Value = client
        .get(format!("{}/usage", second.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["total"], 0);

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

#[tokio::test]
async fn management_token_can_rotate_without_changing_server_identity() {
    let root = TempDir::new().unwrap();
    let old_token = "synthetic-management-token-old";
    let new_token = "synthetic-management-token-new";
    let client = reqwest::Client::new();
    let first = spawn_server_with_token(root.path(), old_token).await;
    let first_health: Value = client
        .get(format!("{}/health", first.origin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = first_health["serverId"].as_str().unwrap().to_string();
    first.task.abort();
    let _ = first.task.await;
    drop(first.state);

    let restarted = spawn_server_with_token(root.path(), new_token).await;
    let restarted_health: Value = client
        .get(format!("{}/health", restarted.origin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restarted_health["serverId"], server_id);
    assert_eq!(
        client
            .get(format!("{}/state", restarted.origin))
            .bearer_auth(old_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(format!("{}/state", restarted.origin))
            .bearer_auth(new_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn client_key_lifecycle_is_versioned_and_secrets_are_one_time() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let input = json!({
        "schemaVersion": 1,
        "label": "Phone",
        "sourceIds": null,
        "accountIds": null,
        "allowedModels": [],
        "excludedModels": [],
        "modelPrefix": null,
        "wireApis": ["responses", "images"]
    });

    let unsupported = client
        .post(format!("{}/keys", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "schemaVersion": 2,
            "label": "Phone",
            "sourceIds": null,
            "accountIds": null,
            "allowedModels": [],
            "excludedModels": [],
            "modelPrefix": null,
            "wireApis": ["responses"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        unsupported.json::<Value>().await.unwrap()["error"]["code"],
        "client_access_schema_unsupported"
    );

    let created = client
        .post(format!("{}/keys", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(created.headers().get(CACHE_CONTROL).unwrap(), "no-store");
    let created: Value = created.json().await.unwrap();
    let key_id = created["key"]["id"].as_str().unwrap();
    let first_secret = created["secret"].as_str().unwrap();
    assert_eq!(created["schemaVersion"], 1);

    let invalid_budget = client
        .patch(format!("{}/keys/{key_id}", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"schemaVersion":1,"softBudgetMicroUsd":0}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_budget.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid_budget.json::<Value>().await.unwrap()["error"]["code"],
        "client_soft_budget_invalid"
    );

    let updated: Value = client
        .patch(format!("{}/keys/{key_id}", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "schemaVersion": 1,
            "label": "Tablet",
            "enabled": false,
            "wireApis": ["images"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["label"], "Tablet");
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["wireApis"], json!(["chat_completions"]));

    let rotated = client
        .post(format!("{}/keys/{key_id}/rotate", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(rotated.headers().get(CACHE_CONTROL).unwrap(), "no-store");
    let rotated: Value = rotated.json().await.unwrap();
    assert_ne!(rotated["secret"].as_str().unwrap(), first_secret);

    let listed: Value = client
        .get(format!("{}/keys", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["schemaVersion"], 1);
    assert_eq!(listed["keys"][0]["id"], key_id);
    assert!(!listed.to_string().contains(first_secret));
    assert!(!listed
        .to_string()
        .contains(rotated["secret"].as_str().unwrap()));

    assert_eq!(
        client
            .delete(format!("{}/keys/{key_id}", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    server.task.abort();
}

#[tokio::test]
async fn user_source_lifecycle_rotates_the_server_secret_and_routes_with_it() {
    let root = TempDir::new().unwrap();
    let (upstream, observed, upstream_task) = spawn_source_lifecycle_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let management_key = "synthetic-management-token-value";
    let first_key = "synthetic-source-key-v1";
    let second_key = "synthetic-source-key-v2";

    let created_response = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "name": "Lifecycle source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": first_key,
            "wireApi": "responses",
            "models": []
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created_response.status(), StatusCode::CREATED);
    let created_text = created_response.text().await.unwrap();
    assert!(!created_text.contains(first_key));
    let created: Value = serde_json::from_str(&created_text).unwrap();
    let source_id = created["id"].as_str().unwrap();
    assert_eq!(created["models"], json!(["gpt-source-lifecycle"]));

    let stats_response = client
        .get(format!("{}/sources/{source_id}/stats", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap();
    assert_eq!(stats_response.status(), StatusCode::OK);
    let stats_text = stats_response.text().await.unwrap();
    assert!(!stats_text.contains(first_key));
    assert_eq!(
        serde_json::from_str::<Value>(&stats_text).unwrap(),
        json!({
            "provider": "unsupported",
            "balanceMicroUsd": null,
            "spentMicroUsd": null,
            "requests": null,
            "totalTokens": null
        })
    );

    let disabled: Value = client
        .patch(format!("{}/sources/{source_id}", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"name":"Lifecycle source edited","enabled":false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(disabled["name"], "Lifecycle source edited");
    assert_eq!(disabled["enabled"], false);

    let rotated_response = client
        .patch(format!("{}/sources/{source_id}", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"apiKey":second_key,"enabled":true}))
        .send()
        .await
        .unwrap();
    let rotated_text = rotated_response.text().await.unwrap();
    assert!(!rotated_text.contains(first_key));
    assert!(!rotated_text.contains(second_key));
    assert_eq!(
        client
            .post(format!("{}/sources/{source_id}/test", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        observed.lock().unwrap().last().map(String::as_str),
        Some("Bearer synthetic-source-key-v2")
    );

    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"sourceIds":[source_id],"inPool":true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{}/gateway/start", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let profile: Value = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let routed_before = observed.lock().unwrap().len();
    assert_eq!(
        client
            .post(format!("{}/v1/responses", server.origin))
            .bearer_auth(profile["secret"].as_str().unwrap())
            .json(&json!({"model":"gpt-source-lifecycle","input":"route"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    {
        let observed = observed.lock().unwrap();
        assert!(observed.len() > routed_before);
        assert_eq!(
            observed.last().map(String::as_str),
            Some("Bearer synthetic-source-key-v2")
        );
    }

    let snapshot_text = client
        .get(format!("{}/state", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!snapshot_text.contains(first_key));
    assert!(!snapshot_text.contains(second_key));
    assert_eq!(
        client
            .delete(format!("{}/sources/{source_id}", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(server
        .state
        .vault
        .load(&format!("source:{source_id}"))
        .unwrap()
        .is_none());
    assert!(server.state.snapshot().unwrap().sources.is_empty());

    server.task.abort();
    upstream_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_gateway_serves_two_hundred_concurrent_requests_and_flushes_usage() {
    const REQUESTS: usize = 200;
    let root = TempDir::new().unwrap();
    let (upstream, load, upstream_task) = spawn_load_upstream(REQUESTS).await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Concurrent upstream",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let source_id = source["id"].as_str().unwrap();
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({"sourceIds": [source_id], "inPool": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let generated: Value = client
        .post(format!("{}/keys", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "schemaVersion": 1,
            "label": "Concurrent client",
            "sourceIds": null,
            "accountIds": null,
            "allowedModels": [],
            "excludedModels": [],
            "modelPrefix": null,
            "wireApis": null
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pool_key = generated["secret"].as_str().unwrap().to_string();

    let start = Arc::new(tokio::sync::Barrier::new(REQUESTS + 1));
    let tasks = (0..REQUESTS)
        .map(|index| {
            let client = client.clone();
            let origin = server.origin.clone();
            let pool_key = pool_key.clone();
            let start = start.clone();
            tokio::spawn(async move {
                start.wait().await;
                client
                    .post(format!("{origin}/v1/responses"))
                    .bearer_auth(pool_key)
                    .json(&json!({
                        "model": "gpt-test",
                        "input": format!("concurrent request {index}")
                    }))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    start.wait().await;
    let responses = tokio::time::timeout(Duration::from_secs(15), async {
        let mut responses = Vec::with_capacity(REQUESTS);
        for task in tasks {
            responses.push(task.await.unwrap());
        }
        responses
    })
    .await
    .expect("200 concurrent requests timed out");

    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    assert_eq!(load.total.load(Ordering::Relaxed), REQUESTS);
    assert_eq!(load.max_active.load(Ordering::Relaxed), REQUESTS);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let usage: Value = client
            .get(format!("{}/usage?page=1&pageSize=1", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if usage["total"].as_u64() == Some(REQUESTS as u64) {
            break;
        }
        assert!(Instant::now() < deadline, "usage queue did not drain");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let persisted: Value = client
        .get(format!(
            "{}/usage?page=1&pageSize={REQUESTS}",
            server.origin
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let events = persisted["events"].as_array().unwrap();
    assert_eq!(events.len(), REQUESTS);
    assert!(events.iter().any(|event| {
        event
            .pointer("/routing/inFlightBefore")
            .and_then(Value::as_u64)
            .is_some_and(|in_flight| in_flight > 0)
    }));
    let snapshot: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!snapshot["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "usage_persistence_failed"));

    server.task.abort();
    upstream_task.abort();
}

#[tokio::test]
async fn adaptive_quota_refresh_has_no_remote_interval_setting() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let removed = client
        .post(format!("{}/quota/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"refreshIntervalSeconds": 120, "requestTimeoutSeconds": 10}))
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NOT_FOUND);
    let state: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(state["gateway"]
        .get("quotaRefreshIntervalSeconds")
        .is_none());
    assert!(state["gateway"].get("useFreeAccounts").is_none());

    let invalid_routing = client
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "maxRetryCandidates": 0
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_routing.status(), StatusCode::BAD_REQUEST);

    let routing: Value = client
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "maxRetryCandidates": 5,
            "routingStrategy": "subscription_plan",
            "subscriptionPlanOrder": ["business", "plus"],
            "imageBaseModel": "gpt-5.4-mini"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(routing["gateway"]["maxRetryCandidates"], 5);
    assert!(routing["gateway"].get("sessionAffinity").is_none());
    assert!(routing["gateway"]
        .get("sessionAffinityTtlSeconds")
        .is_none());
    assert_eq!(routing["gateway"]["routingStrategy"], "subscription_plan");
    assert_eq!(
        routing["gateway"]["subscriptionPlanOrder"],
        json!(["business", "plus"])
    );
    assert_eq!(routing["gateway"]["imageBaseModel"], "gpt-5.4-mini");
    assert!(routing["capabilities"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feature| feature == "runtime_routing"));
    let runtime_order: Value = client
        .get(format!("{}/routing/runtime", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(runtime_order.is_array());

    let refreshed: Value = client
        .post(format!("{}/pool/quota/refresh", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refreshed["refreshed"], 0);
    assert_eq!(refreshed["failed"], 0);
    assert!(refreshed["snapshot"]["gateway"]
        .get("quotaRefreshIntervalSeconds")
        .is_none());
    assert!(refreshed["snapshot"]["gateway"]
        .get("useFreeAccounts")
        .is_none());
    assert_eq!(refreshed["snapshot"]["gateway"]["maxRetryCandidates"], 5);

    server.task.abort();
}

#[tokio::test]
async fn batch_import_accepts_portable_bundles_and_confirms_selected_accounts() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let zenith_preview_response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "content": json!({
                "format": "zenith",
                "version": 1,
                "description": "Seller description",
                "accounts": [{
                    "name": "Zenith account",
                    "provider": "openai",
                    "auth": {
                        "type": "oauth",
                        "accessToken": "synthetic-zenith-access",
                        "refreshToken": "synthetic-zenith-refresh",
                        "expiresAt": "2026-08-19T00:00:00Z"
                    },
                    "identity": {
                        "accountId": "synthetic-zenith-account"
                    },
                    "subscription": {
                        "plan": "business",
                        "expiresAt": "2026-09-19T00:00:00Z"
                    }
                }]
            }).to_string()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(zenith_preview_response.status(), StatusCode::CREATED);
    let zenith_preview_text = zenith_preview_response.text().await.unwrap();
    assert!(!zenith_preview_text.contains("synthetic-zenith-access"));
    assert!(!zenith_preview_text.contains("synthetic-zenith-refresh"));
    let zenith_preview: Value = serde_json::from_str(&zenith_preview_text).unwrap();
    assert_eq!(zenith_preview["preview"]["format"], "zenith_v1");
    assert_eq!(
        zenith_preview["preview"]["description"],
        "Seller description"
    );
    assert_eq!(zenith_preview["preview"]["rows"][0]["plan"], "business");
    let missing_refresh = client
        .post(format!(
            "{}/accounts/account_missing/refresh",
            server.origin
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(missing_refresh.status(), StatusCode::NOT_FOUND);
    let second_id_token = jwt(json!({
        "exp": 1_789_084_800,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "synthetic-batch-account-two",
            "chatgpt_plan_type": "business",
            "chatgpt_subscription_active_until": "2026-10-10T00:00:00Z"
        }
    }));
    let content = json!({
        "version": 1,
        "proxies": [{"password": "synthetic-proxy-secret-never-import"}],
        "sources": [{"apiKey": "synthetic-source-secret-never-import"}],
        "accounts": [
            {
                "name": "Portable first",
                "platform": "openai",
                "type": "oauth",
                "credentials": {
                    "access_token": "synthetic-batch-access-one",
                    "refresh_token": "synthetic-batch-refresh-one",
                    "account_id": "synthetic-batch-account-one",
                    "expires_at": "2026-08-10T00:00:00Z",
                    "plan_type": "plus",
                    "subscription_expires_at": "2026-09-10T00:00:00Z"
                },
                "models": ["gpt-test"]
            },
            {
                "name": "Portable second",
                "tokens": {
                    "accessToken": "synthetic-batch-access-two",
                    "refreshToken": "synthetic-batch-refresh-two",
                    "idToken": second_id_token
                },
                "models": ["gpt-test"]
            }
        ]
    })
    .to_string();

    let preview_response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview_response.status(), StatusCode::CREATED);
    let preview_text = preview_response.text().await.unwrap();
    for secret in [
        "synthetic-batch-access-one",
        "synthetic-batch-refresh-one",
        "synthetic-batch-access-two",
        "synthetic-batch-refresh-two",
        "synthetic-proxy-secret-never-import",
        "synthetic-source-secret-never-import",
        &second_id_token,
    ] {
        assert!(!preview_text.contains(secret));
    }
    let preview: Value = serde_json::from_str(&preview_text).unwrap();
    assert_eq!(preview["preview"]["format"], "portable_account_bundle");
    assert_eq!(preview["preview"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(preview["preview"]["rows"][0]["plan"], "plus");
    assert_eq!(preview["preview"]["warnings"][0]["code"], "proxies_ignored");
    assert_eq!(preview["preview"]["warnings"].as_array().unwrap().len(), 1);
    let batch_id = preview["sessionId"].as_str().unwrap();
    let rows = preview["preview"]["rows"].as_array().unwrap();
    let first_item_id = rows[0]["itemId"].as_str().unwrap();
    let second_item_id = rows[1]["itemId"].as_str().unwrap();

    let first_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(
            &json!({"sessionId": batch_id, "selectedItemIds": [first_item_id], "addToPool": true}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first_confirm["sessionId"], batch_id);
    assert_eq!(first_confirm["results"][0]["status"], "succeeded");
    assert_eq!(server.state.store.accounts().unwrap().len(), 1);
    let first_account = server.state.store.accounts().unwrap().remove(0);
    assert_eq!(first_confirm["results"][0]["accountId"], first_account.id);
    assert!(first_account.in_pool);
    assert_eq!(
        first_account.subscription.plan_type.as_deref(),
        Some("plus")
    );
    assert!(first_account.subscription.active_until_ms.is_some());
    let credential: Value = serde_json::from_str(
        &server
            .state
            .vault
            .load(&first_account.secret_ref)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(credential["expiresAtMs"]
        .as_u64()
        .is_some_and(|value| value > 1_000_000_000_000));

    let second_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": batch_id, "selectedItemIds": [second_item_id]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        second_confirm["results"][0]["status"], "succeeded",
        "{second_confirm}"
    );
    assert_eq!(server.state.store.accounts().unwrap().len(), 2);
    let second_account = server
        .state
        .store
        .accounts()
        .unwrap()
        .into_iter()
        .find(|account| account.label == "Portable second")
        .unwrap();
    assert!(!second_account.in_pool);
    assert_eq!(
        second_account.subscription.plan_type.as_deref(),
        Some("business")
    );
    assert_eq!(
        second_account.subscription.active_until_ms,
        Some(1_791_590_400_000)
    );

    let database = std::fs::read(root.path().join("relay.sqlite")).unwrap();
    let database = String::from_utf8_lossy(&database);
    assert!(!database.contains("synthetic-proxy-secret-never-import"));
    assert!(!database.contains("synthetic-source-secret-never-import"));
    server.task.abort();
}

fn jwt(payload: Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("{header}.{payload}.synthetic-signature")
}

#[tokio::test]
async fn batch_import_accepts_multiple_documents_and_confirms_every_selected_account() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let documents = (1..=3)
        .map(|index| {
            json!({
                "name": format!("Document {index}"),
                "credentials": {
                    "access_token": format!("synthetic-document-access-{index}"),
                    "refresh_token": format!("synthetic-document-refresh-{index}"),
                    "chatgpt_account_id": format!("synthetic-document-account-{index}")
                },
                "models": ["gpt-test"]
            })
            .to_string()
        })
        .collect::<Vec<_>>();

    let preview_response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"documents": documents}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview_response.status(), StatusCode::CREATED);
    let preview_text = preview_response.text().await.unwrap();
    assert!(!preview_text.contains("synthetic-document-access"));
    assert!(!preview_text.contains("synthetic-document-refresh"));
    let preview: Value = serde_json::from_str(&preview_text).unwrap();
    assert_eq!(preview["preview"]["format"], "json_array");
    let rows = preview["preview"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row["defaultSelected"] == true));
    let selected_item_ids = rows
        .iter()
        .map(|row| row["itemId"].as_str().unwrap())
        .collect::<Vec<_>>();

    let confirmed: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": preview["sessionId"],
            "selectedItemIds": selected_item_ids
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(confirmed["results"].as_array().unwrap().len(), 3);
    assert!(confirmed["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["status"] == "succeeded"));
    let accounts = server.state.store.accounts().unwrap();
    assert_eq!(accounts.len(), 3);
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.secret_ref.as_str())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert!(accounts.iter().all(|account| server
        .state
        .vault
        .load(&account.secret_ref)
        .unwrap()
        .is_some()));
    server.task.abort();
}

#[tokio::test]
async fn batch_import_accepts_agent_identity_and_keeps_it_in_the_vault() {
    const PRIVATE_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let content = json!({
        "type": "sub2api-data",
        "version": 1,
        "accounts": [{
            "name": "Agent account",
            "credentials": {
                "auth_mode": "agentIdentity",
                "agent_private_key": PRIVATE_KEY,
                "agent_runtime_id": "runtime-test",
                "task_id": "task-test",
                "chatgpt_account_id": "account-test"
            },
            "models": ["gpt-test"]
        }]
    })
    .to_string();
    let response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let text = response.text().await.unwrap();
    assert!(!text.contains(PRIVATE_KEY));
    let preview: Value = serde_json::from_str(&text).unwrap();
    let item_id = preview["preview"]["rows"][0]["itemId"].as_str().unwrap();
    let confirmed: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": preview["sessionId"],
            "selectedItemIds": [item_id],
            "addToPool": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(confirmed["results"][0]["status"], "succeeded");
    let account = server.state.store.accounts().unwrap().remove(0);
    let stored = server
        .state
        .vault
        .load(&account.secret_ref)
        .unwrap()
        .unwrap();
    assert!(stored.contains(PRIVATE_KEY));
    assert!(!std::fs::read(root.path().join("relay.sqlite"))
        .unwrap()
        .windows(PRIVATE_KEY.len())
        .any(|window| window == PRIVATE_KEY.as_bytes()));
    server.task.abort();
}

#[tokio::test]
async fn batch_import_handles_arrays_json_lines_invalid_rows_and_duplicates() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let valid = json!({
        "label": "Array account",
        "accessToken": "synthetic-array-access",
        "refreshToken": "synthetic-array-refresh",
        "chatgptAccountId": "synthetic-array-account",
        "models": ["gpt-test"]
    });
    let preview = batch_preview(
        &client,
        &server.origin,
        json!([
            valid,
            {"name": "invalid-row-marker"},
            {
                "name": "synthetic-label-secret",
                "accessToken": "synthetic-label-secret",
                "chatgptAccountId": "synthetic-label-account",
                "planType": "synthetic-label-secret"
            }
        ])
        .to_string(),
    )
    .await;
    assert_eq!(preview["preview"]["format"], "json_array");
    assert_eq!(preview["preview"]["rows"][0]["status"], "ready");
    assert_eq!(preview["preview"]["rows"][1]["status"], "invalid");
    assert_eq!(
        preview["preview"]["rows"][1]["error"]["code"],
        "missing_credentials"
    );
    assert!(!preview.to_string().contains("invalid-row-marker"));
    assert!(!preview.to_string().contains("synthetic-label-secret"));
    assert_eq!(preview["preview"]["rows"][2]["label"], "synt...ount");
    assert!(preview["preview"]["rows"][2]["plan"].is_null());

    let batch_id = preview["sessionId"].as_str().unwrap();
    let item_id = preview["preview"]["rows"][0]["itemId"].as_str().unwrap();
    let confirmed = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": batch_id, "selectedItemIds": [item_id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);

    let mut configured = server.state.store.accounts().unwrap().remove(0);
    configured.label = "Server display name".into();
    configured.enabled = false;
    configured.draining = true;
    configured.models = vec!["gpt-server".into()];
    configured.allowed_models = vec!["gpt-server".into()];
    configured.excluded_models = vec!["gpt-blocked".into()];
    configured.priority = 42;
    configured.weight = 7;
    server.state.store.save_account(&configured).unwrap();

    let duplicate = batch_preview(
        &client,
        &server.origin,
        json!({
            "label": "Duplicate account",
            "accessToken": "synthetic-duplicate-access",
            "chatgptAccountId": "synthetic-array-account"
        })
        .to_string(),
    )
    .await;
    assert_eq!(duplicate["preview"]["rows"][0]["status"], "existing");
    assert_eq!(duplicate["preview"]["rows"][0]["defaultSelected"], false);
    let duplicate_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": duplicate["sessionId"],
            "selectedItemIds": [duplicate["preview"]["rows"][0]["itemId"]]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate_confirm["results"][0]["status"], "succeeded");
    let preserved = server.state.store.accounts().unwrap().remove(0);
    assert_eq!(preserved.label, "Server display name");
    assert!(!preserved.enabled);
    assert!(preserved.draining);
    assert_eq!(preserved.models, vec!["gpt-server"]);
    assert_eq!(preserved.allowed_models, vec!["gpt-server"]);
    assert_eq!(preserved.excluded_models, vec!["gpt-blocked"]);
    assert_eq!(preserved.priority, 42);
    assert_eq!(preserved.weight, 7);

    let json_lines = [
        json!({"label":"Line one","accessToken":"synthetic-line-access-one","chatgptAccountId":"synthetic-line-account-one"}).to_string(),
        json!({"label":"Line two","accessToken":"synthetic-line-access-two","chatgptAccountId":"synthetic-line-account-two"}).to_string(),
    ]
    .join("\n");
    let lines_preview = batch_preview(&client, &server.origin, json_lines).await;
    assert_eq!(lines_preview["preview"]["format"], "json_lines");
    assert_eq!(
        lines_preview["preview"]["rows"].as_array().unwrap().len(),
        2
    );
    server.task.abort();
}

#[tokio::test]
async fn batch_import_enforces_size_count_depth_and_batch_ownership() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let oversized = "x".repeat(4 * 1024 * 1024 + 1);
    assert_batch_error(&client, &server.origin, oversized, "import_too_large").await;
    let too_many = Value::Array((0..1_025).map(|_| json!({})).collect()).to_string();
    assert_batch_error(&client, &server.origin, too_many, "import_item_count").await;
    let mut deep = json!({});
    for _ in 0..34 {
        deep = json!({"nested": deep});
    }
    assert_batch_error(&client, &server.origin, deep.to_string(), "import_too_deep").await;

    let first = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-owned-one","chatgptAccountId":"synthetic-owned-account-one"}).to_string(),
    )
    .await;
    let second = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-owned-two","chatgptAccountId":"synthetic-owned-account-two"}).to_string(),
    )
    .await;
    let response: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": first["sessionId"],
            "selectedItemIds": [second["preview"]["rows"][0]["itemId"]]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["results"][0]["status"], "failed");
    assert_eq!(response["results"][0]["error"]["code"], "import_not_found");
    assert!(server.state.store.accounts().unwrap().is_empty());

    let abandoned = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-abandoned","chatgptAccountId":"synthetic-abandoned-account"}).to_string(),
    )
    .await;
    let abandoned_id = abandoned["preview"]["rows"][0]["itemId"].as_str().unwrap();
    let mut pending = server
        .state
        .store
        .pending_import(abandoned_id)
        .unwrap()
        .unwrap();
    pending.created_at_ms = 1;
    let abandoned_secret_ref = pending.secret_ref.clone();
    server.state.store.save_pending_import(&pending).unwrap();
    assert!(server
        .state
        .vault
        .load(&abandoned_secret_ref)
        .unwrap()
        .is_some());
    let _ = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-cleanup-trigger","chatgptAccountId":"synthetic-cleanup-trigger-account"}).to_string(),
    )
    .await;
    assert!(server
        .state
        .store
        .pending_import(abandoned_id)
        .unwrap()
        .is_none());
    assert!(server
        .state
        .vault
        .load(&abandoned_secret_ref)
        .unwrap()
        .is_none());
    server.task.abort();
}

#[tokio::test]
async fn server_account_proxies_support_common_override_bulk_and_redaction() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let (common_address, common_hits, common_task) = spawn_account_proxy("common-proxy").await;
    let (account_address, account_hits, account_task) = spawn_account_proxy("account-proxy").await;
    let client = reqwest::Client::new();
    let common_secret = format!("common-user:common-pass@{common_address}");
    let account_secret = format!("account-user:account-pass@{account_address}");

    let common_state: Value = client
        .post(format!("{}/proxies/common", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": common_secret}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(common_state["gateway"]["commonProxyConfigured"], true);
    assert_eq!(common_state["gateway"]["commonProxyAvailable"], true);
    assert!(!common_state.to_string().contains("common-pass"));

    let preview: Value = client
        .post(format!("{}/accounts/import/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Proxy account",
            "accessToken": "synthetic-proxy-access-token",
            "expiresAtMs": 4_000_000_000_000_u64,
            "chatgptAccountId": "synthetic-proxy-account-id",
            "responsesUrl": "http://127.0.0.1:9/account/responses",
            "models": ["gpt-proxy-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let account_id = preview["accountId"].as_str().unwrap().to_string();
    let confirmed: Value = client
        .post(format!("{}/accounts/import/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": preview["sessionId"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(confirmed["proxyMode"], "common");
    assert_eq!(confirmed["proxyAvailable"], true);
    let membership = client
        .post(format!("{}/pool/members", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"accountIds": [account_id], "inPool": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);

    let generated: Value = client
        .post(format!("{}/keys", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "schemaVersion": 1,
            "label": "Proxy test client",
            "sourceIds": [],
            "accountIds": [account_id],
            "allowedModels": [],
            "excludedModels": [],
            "modelPrefix": null,
            "wireApis": null
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pool_key = generated["secret"].as_str().unwrap();

    let first = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"common proxy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert!(first.text().await.unwrap().contains("common-proxy"));
    assert_eq!(common_hits.load(Ordering::SeqCst), 1);

    let account_state: Value = client
        .post(format!("{}/accounts/{account_id}/proxy", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": account_secret}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(account_state["proxyMode"], "account");
    assert!(!account_state.to_string().contains("account-pass"));

    let second = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"account proxy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert!(second.text().await.unwrap().contains("account-proxy"));
    assert_eq!(account_hits.load(Ordering::SeqCst), 1);

    let replacement_preview: Value = client
        .post(format!("{}/accounts/import/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Proxy account refreshed",
            "accessToken": "synthetic-proxy-access-token-refreshed",
            "expiresAtMs": 4_000_000_100_000_u64,
            "chatgptAccountId": "synthetic-proxy-account-id",
            "responsesUrl": "http://127.0.0.1:9/account/responses",
            "models": ["gpt-proxy-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replacement_preview["accountId"], account_id);
    let replacement: Value = client
        .post(format!("{}/accounts/import/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": replacement_preview["sessionId"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replacement["proxyMode"], "account");
    assert!(!replacement.to_string().contains("account-pass"));

    let bulk: Value = client
        .post(format!("{}/accounts/proxies/assign", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "accountIds": [account_id],
            "proxyUrls": [account_secret, "unused:unused@127.0.0.1:9999"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bulk, json!({"assigned": 1, "unused": 1}));

    let inherited: Value = client
        .post(format!("{}/accounts/{account_id}/proxy", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(inherited["proxyMode"], "common");
    let third = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"inherited proxy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::OK);
    assert!(third.text().await.unwrap().contains("common-proxy"));
    assert_eq!(common_hits.load(Ordering::SeqCst), 2);

    let state_text = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for secret in ["common-pass", "account-pass", "unused:unused"] {
        assert!(!state_text.contains(secret));
    }
    let database_bytes = std::fs::read(root.path().join("relay.sqlite")).unwrap();
    let database = String::from_utf8_lossy(&database_bytes);
    let vault_bytes = std::fs::read(root.path().join("vault").join("secrets.enc")).unwrap();
    let vault = String::from_utf8_lossy(&vault_bytes);
    for secret in ["common-pass", "account-pass", "unused:unused"] {
        assert!(!database.contains(secret));
        assert!(!vault.contains(secret));
    }

    let common_proxy = server
        .state
        .store
        .proxy(&server.state.store.common_proxy_id().unwrap().unwrap())
        .unwrap()
        .unwrap();
    server.state.vault.delete(&common_proxy.secret_ref).unwrap();
    server.task.abort();
    let _ = server.task.await;
    drop(server.state);

    let recovered = spawn_server(root.path()).await;
    let recovery_state: Value = client
        .get(format!("{}/state", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovery_state["gateway"]["commonProxyConfigured"], true);
    assert_eq!(recovery_state["gateway"]["commonProxyAvailable"], false);
    assert_eq!(recovery_state["gateway"]["running"], false);
    assert_eq!(recovery_state["accounts"][0]["proxyMode"], "common");
    assert_eq!(recovery_state["accounts"][0]["proxyAvailable"], false);

    let repaired_state: Value = client
        .post(format!("{}/proxies/common", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": common_secret}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repaired_state["gateway"]["commonProxyAvailable"], true);
    assert_eq!(repaired_state["gateway"]["running"], true);

    let strict_state: Value = client
        .post(format!("{}/proxies/policy", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"required": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(strict_state["gateway"]["accountProxyRequired"], true);
    let blocked_state: Value = client
        .post(format!("{}/proxies/common", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(blocked_state["gateway"]["commonProxyConfigured"], false);
    assert_eq!(blocked_state["gateway"]["accountProxyRequired"], true);
    assert_eq!(blocked_state["accounts"][0]["proxyMode"], "direct");
    assert_eq!(blocked_state["accounts"][0]["proxyAvailable"], false);
    let blocked_request = client
        .post(format!("{}/v1/responses", recovered.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"must not use direct egress"}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_request.status(), StatusCode::SERVICE_UNAVAILABLE);

    recovered.task.abort();
    common_task.abort();
    account_task.abort();
}

#[tokio::test]
async fn configuration_presets_preview_apply_reject_stale_and_exclude_secrets() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Preset source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let source_id = source["id"].as_str().unwrap();
    let proxy_secret = "http://preset-user:preset-pass@127.0.0.1:9";
    let proxy_state = client
        .post(format!("{}/proxies/common", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": proxy_secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(proxy_state.status(), StatusCode::OK);

    let account_preview: Value = client
        .post(format!("{}/accounts/import/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Preset account",
            "accessToken": "synthetic-preset-access-token",
            "expiresAtMs": 4_000_000_000_000_u64,
            "chatgptAccountId": "synthetic-preset-account-id",
            "responsesUrl": format!("{upstream}/v1/responses"),
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let account_id = account_preview["accountId"].as_str().unwrap();
    let account_confirm = client
        .post(format!("{}/accounts/import/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": account_preview["sessionId"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(account_confirm.status(), StatusCode::OK);
    let account_proxy = client
        .post(format!("{}/accounts/{account_id}/proxy", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": proxy_secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(account_proxy.status(), StatusCode::OK);

    let document_response = client
        .get(format!("{}/configuration/preset", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(document_response.status(), StatusCode::OK);
    let document_text = document_response.text().await.unwrap();
    for excluded in [
        "synthetic-upstream-api-key",
        "synthetic-preset-access-token",
        "preset-pass",
        "managementToken",
        "clientKey",
        "vault",
        "usage",
        "publicBaseUrl",
    ] {
        assert!(!document_text.contains(excluded), "{excluded}");
    }
    let document: Value = serde_json::from_str(&document_text).unwrap();
    assert_eq!(document["preset"]["format"], "zenith-relay-configuration");
    assert_eq!(document["preset"]["schemaVersion"], 2);
    assert!(document["revision"]
        .as_str()
        .is_some_and(|revision| revision.starts_with("cfg_")));
    assert!(document["preset"]["settings"]["quota"]["commonProxyId"]
        .as_str()
        .is_some_and(|proxy_id| proxy_id.starts_with("proxy_")));

    let mut preset = document["preset"].clone();
    let source_rule = preset["settings"]["sources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|rule| rule["id"] == source_id)
        .unwrap();
    source_rule["inPool"] = json!(true);
    source_rule["priority"] = json!(7);
    source_rule["id"] = json!("source_local_record");
    let account_rule = preset["settings"]["accounts"]
        .as_array_mut()
        .unwrap()
        .first_mut()
        .unwrap();
    account_rule["id"] = json!("account_local_record");
    account_rule["inPool"] = json!(true);
    account_rule["priority"] = json!(9);
    preset["settings"]["routing"]["maxRetryCandidates"] = json!(4);
    preset["settings"]["hiddenModels"] = json!(["gpt-test"]);

    let preview: Value = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": preset}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let paths = preview["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(preview["preset"]["settings"]["sources"][0]["id"], source_id);
    assert_eq!(
        preview["preset"]["settings"]["accounts"][0]["id"],
        account_id
    );
    assert!(paths.contains("/sources/0/inPool"));
    assert!(paths.contains("/sources/0/priority"));
    assert!(paths.contains("/accounts/0/inPool"));
    assert!(paths.contains("/accounts/0/priority"));
    assert!(paths.contains("/routing/maxRetryCandidates"));
    assert!(paths.contains("/hiddenModels"));
    let priority_change = preview["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == "/sources/0/priority")
        .unwrap();
    assert_eq!(priority_change["before"], 0);
    assert_eq!(priority_change["after"], 7);

    let routing_change = client
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"maxRetryCandidates": 5, "routingStrategy": "adaptive"}))
        .send()
        .await
        .unwrap();
    assert_eq!(routing_change.status(), StatusCode::OK);
    let stale = client
        .post(format!("{}/configuration/preset/apply", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "baseRevision": preview["baseRevision"],
            "preset": preview["preset"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let stale_body: Value = stale.json().await.unwrap();
    assert_eq!(stale_body["error"]["code"], "configuration_revision_stale");
    let unchanged: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged["gateway"]["maxRetryCandidates"], 5);
    assert_eq!(unchanged["sources"][0]["priority"], 0);
    assert_eq!(unchanged["sources"][0]["inPool"], false);

    let fresh_preview: Value = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": preview["preset"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let applied: Value = client
        .post(format!("{}/configuration/preset/apply", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "baseRevision": fresh_preview["baseRevision"],
            "preset": fresh_preview["preset"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(applied["previousRevision"], applied["revision"]);
    let applied_state: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(applied_state["configurationRevision"], applied["revision"]);
    assert_eq!(applied_state["gateway"]["maxRetryCandidates"], 4);
    assert_eq!(applied_state["sources"][0]["priority"], 7);
    assert_eq!(applied_state["sources"][0]["inPool"], true);
    assert_eq!(applied_state["accounts"][0]["priority"], 9);
    assert_eq!(applied_state["accounts"][0]["inPool"], true);
    assert_eq!(
        applied_state["accounts"][0]["proxyId"],
        document["preset"]["settings"]["quota"]["commonProxyId"]
    );
    assert_eq!(applied_state["gateway"]["visibleModelIds"], json!([]));

    let mut unsupported_schema = fresh_preview["preset"].clone();
    unsupported_schema["schemaVersion"] = json!(3);
    let unsupported = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": unsupported_schema}))
        .send()
        .await
        .unwrap();
    assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);

    let mut unknown_field = fresh_preview["preset"].clone();
    unknown_field["settings"]["unexpected"] = json!(true);
    let unknown = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": unknown_field}))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut missing = fresh_preview["preset"].clone();
    missing["settings"]["sources"][0]["id"] = json!("source_missing");
    missing["settings"]["sources"][0]["baseUrl"] = json!("https://missing.invalid/v1");
    let missing_response = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": missing}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let missing_body: Value = missing_response.json().await.unwrap();
    assert_eq!(
        missing_body["error"]["code"],
        "configuration_reference_missing"
    );
    let after_failures: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after_failures["configurationRevision"], applied["revision"]);

    server.task.abort();
    let _ = server.task.await;
    drop(server.state);
    let restarted = spawn_server(root.path()).await;
    let restarted_state: Value = client
        .get(format!("{}/state", restarted.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        restarted_state["configurationRevision"],
        applied["revision"]
    );
    assert_eq!(restarted_state["gateway"]["maxRetryCandidates"], 4);
    assert_eq!(restarted_state["sources"][0]["priority"], 7);
    assert_eq!(restarted_state["accounts"][0]["priority"], 9);
    assert_eq!(restarted_state["accounts"][0]["inPool"], true);
    restarted.task.abort();
    upstream_task.abort();
}

async fn batch_preview(client: &reqwest::Client, origin: &str, content: String) -> Value {
    let response = client
        .post(format!("{origin}/accounts/import/batch/preview"))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

async fn assert_batch_error(
    client: &reqwest::Client,
    origin: &str,
    content: String,
    expected_code: &str,
) {
    let response = client
        .post(format!("{origin}/accounts/import/batch/preview"))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], expected_code);
}

struct RunningServer {
    origin: String,
    state: Arc<AppState>,
    task: tokio::task::JoinHandle<()>,
}

async fn spawn_server(root: &Path) -> RunningServer {
    spawn_server_with_token(root, "synthetic-management-token-value").await
}

async fn spawn_server_with_token(root: &Path, management_token: &str) -> RunningServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = Config {
        bind: address,
        data_dir: root.to_path_buf(),
        public_base_url: url::Url::parse(&format!("http://{address}")).unwrap(),
        management_token: management_token.to_string(),
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
        .route("/v1/models", get(models_response))
        .route("/v1/responses", post(upstream_response))
        .route("/account/responses", post(account_response))
        .route("/account/responses/compact", post(account_compact))
        .route("/account/alpha/search", post(account_search));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn spawn_mixed_protocol_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route("/v1/models", get(mixed_protocol_models));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn mixed_protocol_models(request: Request) -> impl IntoResponse {
    if request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer synthetic-upstream-api-key")
    {
        return Json(json!({"data":[{"id":"gpt-native"}]})).into_response();
    }
    if request
        .headers()
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        == Some("synthetic-upstream-api-key")
        && request
            .headers()
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            == Some("2023-06-01")
    {
        return Json(json!({"data":[{"id":"claude-native"}]})).into_response();
    }
    StatusCode::UNAUTHORIZED.into_response()
}

#[derive(Clone, Debug)]
struct NativeMessagesRequest {
    x_api_key: Option<String>,
    anthropic_version: Option<String>,
    anthropic_beta: Option<String>,
    body: Value,
}

async fn spawn_messages_upstream() -> (
    String,
    Arc<Mutex<Vec<NativeMessagesRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/v1/models", get(native_messages_models))
        .route("/v1/messages", post(native_messages_response))
        .with_state(requests.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), requests, task)
}

async fn native_messages_models(
    State(requests): State<Arc<Mutex<Vec<NativeMessagesRequest>>>>,
    request: Request,
) -> impl IntoResponse {
    let headers = request.headers();
    requests.lock().unwrap().push(NativeMessagesRequest {
        x_api_key: header_value(headers, "x-api-key"),
        anthropic_version: header_value(headers, "anthropic-version"),
        anthropic_beta: header_value(headers, "anthropic-beta"),
        body: Value::Null,
    });
    Json(json!({"data":[{"id":"claude-native"}]}))
}

async fn native_messages_response(
    State(requests): State<Arc<Mutex<Vec<NativeMessagesRequest>>>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body["tools"].is_array());
    assert_eq!(body["tools"][0]["name"], "read_file");
    requests.lock().unwrap().push(NativeMessagesRequest {
        x_api_key: header_value(&parts.headers, "x-api-key"),
        anthropic_version: header_value(&parts.headers, "anthropic-version"),
        anthropic_beta: header_value(&parts.headers, "anthropic-beta"),
        body,
    });
    (
        StatusCode::OK,
        Json(json!({
            "id": "msg_native",
            "type": "message",
            "role": "assistant",
            "model": "claude-native",
            "content": [{
                "type": "tool_use",
                "id": "toolu_native",
                "name": "read_file",
                "input": {"path": "README.md"}
            }],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })),
    )
        .into_response()
}

fn header_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn spawn_source_lifecycle_upstream(
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/v1/models", get(source_lifecycle_models))
        .route("/v1/responses", post(source_lifecycle_response))
        .with_state(observed.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), observed, task)
}

async fn source_lifecycle_models(
    State(observed): State<Arc<Mutex<Vec<String>>>>,
    request: Request,
) -> impl IntoResponse {
    observe_source_authorization(&observed, &request);
    Json(json!({"data":[{"id":"gpt-source-lifecycle"}]}))
}

async fn source_lifecycle_response(
    State(observed): State<Arc<Mutex<Vec<String>>>>,
    request: Request,
) -> impl IntoResponse {
    observe_source_authorization(&observed, &request);
    Json(json!({
        "id":"response-source-lifecycle",
        "object":"response",
        "model":"gpt-source-lifecycle",
        "output":[],
        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
    }))
}

fn observe_source_authorization(observed: &Mutex<Vec<String>>, request: &Request) {
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    observed.lock().unwrap().push(authorization);
}

#[derive(Clone)]
struct LoadUpstreamState {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    barrier: Arc<tokio::sync::Barrier>,
}

async fn spawn_load_upstream(
    requests: usize,
) -> (String, LoadUpstreamState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = LoadUpstreamState {
        active: Arc::new(AtomicUsize::new(0)),
        max_active: Arc::new(AtomicUsize::new(0)),
        total: Arc::new(AtomicUsize::new(0)),
        barrier: Arc::new(tokio::sync::Barrier::new(requests)),
    };
    let router = Router::new()
        .route("/v1/models", get(models_response))
        .route("/v1/responses", post(load_upstream_response))
        .with_state(state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), state, task)
}

async fn spawn_account_proxy(
    response_id: &'static str,
) -> (SocketAddr, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let marker = hits.clone();
    let router = Router::new().fallback(any(move |request: Request| {
        let marker = marker.clone();
        async move {
            marker.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.method(), axum::http::Method::POST);
            assert!(request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("Bearer synthetic-proxy-access-token")));
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(format!(
                    "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"{response_id}\",\"object\":\"response\",\"model\":\"gpt-proxy-test\",\"output\":[],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n"
                )))
                .unwrap()
        }
    }));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, hits, task)
}

async fn models_response(request: Request) -> impl IntoResponse {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    Json(json!({"data":[{"id":"gpt-test"}]}))
}

async fn upstream_response(request: Request) -> Response {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let stream = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        .unwrap_or(false);
    if stream {
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from("data: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"))
            .unwrap();
    }
    (
        StatusCode::OK,
        Json(json!({
            "id":"response-test",
            "object":"response",
            "model":"gpt-test",
            "error": null,
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        })),
    )
        .into_response()
}

async fn load_upstream_response(
    State(state): State<LoadUpstreamState>,
    request: Request,
) -> Response {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    let _ = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let active = state.active.fetch_add(1, Ordering::Relaxed) + 1;
    state.max_active.fetch_max(active, Ordering::Relaxed);
    state.total.fetch_add(1, Ordering::Relaxed);
    state.barrier.wait().await;
    state.active.fetch_sub(1, Ordering::Relaxed);
    (
        StatusCode::OK,
        Json(json!({
            "id":"concurrent-response",
            "object":"response",
            "model":"gpt-test",
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        })),
    )
        .into_response()
}

async fn account_response(request: Request) -> Response {
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
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert!(body["input"].is_array());
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"message\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"account-response-test\",\"object\":\"response\",\"model\":\"gpt-test\",\"output\":[],\"usage\":{\"input_tokens\":1,\"input_tokens_details\":{\"cached_tokens\":1},\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ))
        .unwrap()
}

async fn account_compact(request: Request) -> Response {
    assert_account_authorization(&request);
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body.get("stream").is_none());
    Json(json!({"type":"compaction","items":[]})).into_response()
}

async fn account_search(request: Request) -> Response {
    assert_account_authorization(&request);
    assert_eq!(
        request
            .headers()
            .get("x-session-id")
            .and_then(|value| value.to_str().ok()),
        Some("remote-session")
    );
    Json(json!({"results":[{"title":"remote result"}]})).into_response()
}

fn assert_account_authorization(request: &Request) {
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
}

async fn assert_websocket_upgrade(origin: &str, key: &str) {
    assert_websocket_status(origin, key, "101").await;
}

async fn assert_websocket_status(origin: &str, key: &str, status: &str) {
    let address = origin.strip_prefix("http://").unwrap();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET /v1/responses HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {key}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = [0_u8; 1024];
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8_lossy(&response[..read]);
    assert!(
        response.starts_with(&format!("HTTP/1.1 {status} ")),
        "{response}"
    );
}

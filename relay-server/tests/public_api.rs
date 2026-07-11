use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    net::SocketAddr,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tempfile::TempDir;
use zenith_relay_server::{
    config::Config,
    http,
    state::{AppState, COMMON_PROXY_SECRET_REF},
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
    assert_eq!(source_response.status(), StatusCode::CREATED);
    let source_text = source_response.text().await.unwrap();
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
    assert!(reopened_usage["total"]
        .as_u64()
        .is_some_and(|total| total >= 4));
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
async fn batch_import_accepts_portable_bundles_and_confirms_selected_accounts() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
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
                    "chatgptAccountId": "synthetic-batch-account-two"
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
    ] {
        assert!(!preview_text.contains(secret));
    }
    let preview: Value = serde_json::from_str(&preview_text).unwrap();
    assert_eq!(preview["preview"]["format"], "portable_account_bundle");
    assert_eq!(preview["preview"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(preview["preview"]["rows"][0]["plan"], "plus");
    assert_eq!(preview["preview"]["warnings"][0]["code"], "proxies_ignored");
    assert_eq!(preview["preview"]["warnings"][1]["code"], "sources_ignored");
    let batch_id = preview["sessionId"].as_str().unwrap();
    let rows = preview["preview"]["rows"].as_array().unwrap();
    let first_item_id = rows[0]["itemId"].as_str().unwrap();
    let second_item_id = rows[1]["itemId"].as_str().unwrap();

    let first_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": batch_id, "selectedItemIds": [first_item_id]}))
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
    assert_eq!(second_confirm["results"][0]["status"], "succeeded");
    assert_eq!(server.state.store.accounts().unwrap().len(), 2);

    let database = std::fs::read(root.path().join("relay.sqlite")).unwrap();
    let database = String::from_utf8_lossy(&database);
    assert!(!database.contains("synthetic-proxy-secret-never-import"));
    assert!(!database.contains("synthetic-source-secret-never-import"));
    server.task.abort();
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
    assert_eq!(preview["preview"]["format"], "json_documents");
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
    assert_eq!(preview["preview"]["rows"][2]["label"], "Imported account");
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

    let oversized = "x".repeat(1024 * 1024 + 1);
    assert_batch_error(&client, &server.origin, oversized, "import_too_large").await;
    let too_many = Value::Array((0..257).map(|_| json!({})).collect()).to_string();
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

    let generated: Value = client
        .post(format!("{}/keys", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Proxy test client",
            "sourceIds": [],
            "accountIds": [account_id],
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

    server.state.vault.delete(COMMON_PROXY_SECRET_REF).unwrap();
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

    recovered.task.abort();
    common_task.abort();
    account_task.abort();
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
        .route("/v1/models", get(models_response))
        .route("/v1/responses", post(upstream_response))
        .route("/account/responses", post(account_response));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
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
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"message\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"account-response-test\",\"object\":\"response\",\"model\":\"gpt-test\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ))
        .unwrap()
}

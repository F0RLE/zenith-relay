use super::*;

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
    assert!(capabilities["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feature| feature == "account_batch_import_creation_status"));
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
    let pool_key = profile_key.clone();

    let chat = client
        .post(format!("{}/v1/chat/completions", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({
            "model": "gpt-test",
            "messages": [{"role": "user", "content": "test"}]
        }))
        .send()
        .await
        .unwrap();
    let chat_status = chat.status();
    let chat_body = chat.json::<Value>().await.unwrap();
    assert_eq!(chat_status, StatusCode::OK, "{chat_body}");
    assert_eq!(chat_body["object"], "chat.completion");

    let image = client
        .post(format!("{}/v1/images/generations", first.origin))
        .bearer_auth(&pool_key)
        .json(&json!({"model": "gpt-image-2", "prompt": "test"}))
        .send()
        .await
        .unwrap();
    let image_status = image.status();
    let image_body = image.json::<Value>().await.unwrap();
    assert_eq!(image_status, StatusCode::NOT_FOUND, "{image_body}");
    assert_eq!(image_body["error"]["code"], "model_not_found");

    let models = client
        .get(format!("{}/v1/models", first.origin))
        .bearer_auth(&pool_key)
        .send()
        .await
        .unwrap();
    assert_eq!(models.status(), StatusCode::OK);
    assert!(models.text().await.unwrap().contains("gpt-test"));
    assert_websocket_upgrade(&first.origin, &pool_key).await;

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
    let mut exhausted_account = first.state.store.account(account_id).unwrap().unwrap();
    exhausted_account.in_pool = true;
    exhausted_account.quota.limit_reached = true;
    first.state.store.save_account(&exhausted_account).unwrap();
    first.state.rebuild_runtime().await.unwrap();
    let account_runtime = first.state.runtime().unwrap().unwrap();
    assert!(
        !account_runtime
            .candidate_runtime_order()
            .iter()
            .find(|candidate| candidate.candidate_id == account_id)
            .unwrap()
            .available
    );
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
    assert!(Arc::ptr_eq(
        &account_runtime,
        &first.state.runtime().unwrap().unwrap()
    ));
    let reenabling = client
        .patch(format!("{}/accounts/{account_id}", first.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"enabled": true, "draining": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(reenabling.status(), StatusCode::OK);
    let updated_account_runtime = first.state.runtime().unwrap().unwrap();
    assert!(Arc::ptr_eq(&account_runtime, &updated_account_runtime));
    assert!(
        !updated_account_runtime
            .candidate_runtime_order()
            .iter()
            .find(|candidate| candidate.candidate_id == account_id)
            .unwrap()
            .available
    );
    // The unavailable quota state above is only used to prove that a policy
    // hot update preserves scheduler availability. Restore the fixture before
    // this integration scenario verifies that the OAuth route is selected.
    let mut routable_account = first.state.store.account(account_id).unwrap().unwrap();
    routable_account.quota.limit_reached = false;
    first.state.store.save_account(&routable_account).unwrap();
    first.state.rebuild_runtime().await.unwrap();
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
    // Global API-source price rules must not reprice personal-account usage.
    assert_eq!(repriced_usage["totals"]["apiEquivalent"]["microUsd"], 0);
    assert_eq!(repriced_usage["totals"]["apiEquivalent"]["pricedTokens"], 0);
    assert_eq!(
        repriced_usage["totals"]["apiEquivalent"]["unpricedTokens"],
        2
    );
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

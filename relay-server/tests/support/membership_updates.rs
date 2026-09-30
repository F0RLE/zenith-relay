use super::*;

#[tokio::test]
async fn reference_reasoning_reaches_codex_and_manual_policy_only_narrows_it() {
    use sha2::{Digest, Sha256};
    let root = TempDir::new().unwrap();
    // Load a validated reference fixture before server startup. Participant
    // /models declares low/medium/high; the reference instead offers minimal.
    let payload = json!({"openai/gpt-test":{
        "reasoning":true, "reasoning_effort_levels":["minimal","medium","high"],
        "default_reasoning_effort":"medium"
    }});
    let revision = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&payload).unwrap()))
    );
    std::fs::write(
        root.path().join("models-dev.json"),
        json!({
            "format":"zenith-relay-model-metadata-cache", "schemaVersion":1,
            "sourceUrl":zenith_relay_core::model_metadata::MODELS_DEV_SOURCE_URL,
            "revision":revision, "etag":null, "lastModified":null,
            "fetchedAtMs":1, "payloadSha256":revision, "stale":true, "payload":payload
        })
        .to_string(),
    )
    .unwrap();
    let (upstream, upstream_task) = spawn_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let management_key = "synthetic-management-token-value";

    let created: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "name": "Reasoning source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "models": []
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["models"], json!(["gpt-test"]));
    let source_id = created["id"].as_str().unwrap();
    let membership = client
        .post(format!("{}/pool/members", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"sourceIds": [source_id], "inPool": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);
    let profile: Value = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pool_key = profile["secret"].as_str().unwrap();
    let catalog: Value = client
        .get(format!("{}/v1/models?client_version=1.0.0", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let catalog_model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == "gpt-test")
        .unwrap();
    assert_eq!(
        catalog_model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level["effort"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["minimal", "medium", "high"]
    );
    assert_eq!(catalog_model["default_reasoning_level"], "medium");

    let runtime = server.state.runtime().unwrap().unwrap();
    let configured_response = client
        .post(format!("{}/models/reasoning", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"modelId": "gpt-test", "allowedLevels": ["HIGH"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(configured_response.status(), StatusCode::OK);
    let configured: Value = configured_response.json().await.unwrap();
    let configured_model = configured["gateway"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "gpt-test")
        .unwrap();
    assert_eq!(configured_model["reasoningAllowedLevels"], json!(["high"]));
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));
    assert_eq!(
        server
            .state
            .store
            .model_reasoning_allowed_levels()
            .unwrap()
            .get("group:openai"),
        Some(&vec!["high".to_string()])
    );

    let filtered_catalog: Value = client
        .get(format!("{}/v1/models?client_version=1.0.0", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let filtered_model = filtered_catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == "gpt-test")
        .unwrap();
    assert_eq!(
        filtered_model["supported_reasoning_levels"],
        json!([{"effort":"high", "description":"high"}])
    );
    assert_eq!(filtered_model["default_reasoning_level"], "high");

    let manual = client
        .post(format!("{}/models/reasoning", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"modelId": "gpt-test", "allowedLevels": ["ultra"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(manual.status(), StatusCode::OK);
    assert_eq!(
        manual.json::<Value>().await.unwrap()["gateway"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == "gpt-test")
            .unwrap()["reasoningAllowedLevels"],
        json!([])
    );

    let reset: Value = client
        .post(format!("{}/models/reasoning", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"modelId": "gpt-test", "allowedLevels": []}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reset_model = reset["gateway"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "gpt-test")
        .unwrap();
    assert_eq!(reset_model["reasoningAllowedLevels"], json!([]));
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));

    let cleared_catalog: Value = client
        .get(format!("{}/v1/models?client_version=1.0.0", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cleared_model = cleared_catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == "gpt-test")
        .unwrap();
    assert!(cleared_model.get("default_reasoning_level").is_none());
    assert_eq!(cleared_model["supported_reasoning_levels"], json!([]));

    server.task.abort();
    upstream_task.abort();
}

#[tokio::test]
async fn un_draining_source_hot_updates_the_internal_key_scope() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_scope_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let management_key = "synthetic-management-token-value";

    let active: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "name": "Active scope source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "allowedModels": ["gpt-active"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let active_id = active["id"].as_str().unwrap().to_string();
    let draining: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "name": "Draining scope source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "allowedModels": ["gpt-draining"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let draining_id = draining["id"].as_str().unwrap().to_string();

    assert_eq!(
        client
            .patch(format!("{}/sources/{draining_id}", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"draining": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"sourceIds": [active_id.as_str(), draining_id.as_str()], "inPool": true}))
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
    let pool_key = profile["secret"].as_str().unwrap();
    let before: Value = client
        .get(format!("{}/v1/models", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let before_ids = before["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<HashSet<_>>();
    assert!(before_ids.contains("gpt-active"));
    assert!(!before_ids.contains("gpt-draining"));

    let runtime = server.state.runtime().unwrap().unwrap();
    assert_eq!(
        client
            .patch(format!("{}/sources/{draining_id}", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"draining": false}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));

    let after: Value = client
        .get(format!("{}/v1/models", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let after_ids = after["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<HashSet<_>>();
    assert!(after_ids.contains("gpt-draining"));
    assert_eq!(
        client
            .post(format!("{}/v1/responses", server.origin))
            .bearer_auth(pool_key)
            .json(&json!({"model":"gpt-draining","input":"scope refresh"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"sourceIds": [draining_id.as_str()], "inPool": false}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));
    let removed: Value = client
        .get(format!("{}/v1/models", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!removed["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .any(|model| model == "gpt-draining"));

    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"sourceIds": [draining_id.as_str()], "inPool": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));

    server.task.abort();
    upstream_task.abort();
}

#[tokio::test]
async fn rejoining_source_immediately_uses_saved_order_and_shared_capacity_limit() {
    use zenith_relay_core::{
        PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy,
    };
    let root = TempDir::new().unwrap();
    let (primary_url, primary_state, primary_task) = spawn_load_upstream(2).await;
    let (fallback_url, fallback_task) = spawn_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let mut ids = Vec::new();
    for (name, upstream) in [("Primary", primary_url), ("Fallback", fallback_url)] {
        let response = client.post(format!("{}/sources", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({ "name": name, "baseUrl": format!("{upstream}/v1"),
                "apiKey": "synthetic-upstream-api-key", "wireApi": "responses", "models": ["gpt-test"] }))
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let source: Value = response.json().await.unwrap();
        ids.push(source["id"].as_str().unwrap().to_owned());
    }
    let update_membership = |id: &str| {
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({ "sourceIds": [id], "inPool": true }))
            .send()
    };
    assert_eq!(
        update_membership(&ids[1]).await.unwrap().status(),
        StatusCode::OK
    );
    let mut routing = server.state.store.routing_policy().unwrap();
    routing.pool_routing = Some(PoolRoutingPolicy {
        mode: PoolRoutingMode::InOrder,
        members: ids
            .iter()
            .map(|id| PoolRoutingMember {
                kind: PoolMemberKind::Source,
                id: id.clone(),
                weight: 1,
                max_concurrency: 1,
            })
            .collect(),
        ..Default::default()
    });
    server.state.store.set_routing_policy(&routing).unwrap();
    server.state.rebuild_runtime().await.unwrap();
    let runtime = server.state.runtime().unwrap().unwrap();
    let response = update_membership(&ids[0]).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));
    let order = runtime.candidate_runtime_order_for_key("key_system");
    assert_eq!(
        order
            .iter()
            .find(|candidate| candidate.next_for_new_request)
            .unwrap()
            .candidate_id,
        ids[0]
    );
    let profile: Value = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let request = || {
        client
            .post(format!("{}/v1/responses", server.origin))
            .bearer_auth(profile["secret"].as_str().unwrap())
            .json(&json!({ "model": "gpt-test", "input": "synthetic rotation test" }))
            .send()
    };
    let first = tokio::spawn(request());
    tokio::time::timeout(Duration::from_secs(5), async {
        while primary_state.total.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("first request must use the newly joined primary");
    let second = tokio::time::timeout(Duration::from_secs(5), request())
        .await
        .expect("busy primary must fall back immediately")
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(primary_state.total.load(Ordering::Relaxed), 1);
    primary_state.barrier.wait().await;
    assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);
    server.state.shutdown_runtime().await.unwrap();
    server.task.abort();
    primary_task.abort();
    fallback_task.abort();
}

#[tokio::test]
async fn un_draining_account_hot_updates_the_internal_key_scope() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let management_key = "synthetic-management-token-value";

    let source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "name": "Active account-scope source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "allowedModels": ["gpt-test"]
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
            .bearer_auth(management_key)
            .json(&json!({"sourceIds": [source_id], "inPool": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let preview: Value = client
        .post(format!("{}/accounts/import/preview", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "label": "Draining account scope",
            "accessToken": "synthetic-access-token",
            "refreshToken": "synthetic-refresh-token",
            "expiresAtMs": 4_000_000_000_000_u64,
            "chatgptAccountId": "synthetic-chatgpt-account-id",
            "responsesUrl": format!("{upstream}/account/responses"),
            "models": ["gpt-account-only"],
            "priority": 10,
            "weight": 1
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let account: Value = client
        .post(format!("{}/accounts/import/confirm", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"sessionId": preview["sessionId"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let account_id = account["id"].as_str().unwrap().to_string();
    assert_eq!(
        client
            .patch(format!("{}/accounts/{account_id}", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"draining": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"accountIds": [account_id.as_str()], "inPool": true}))
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
    let pool_key = profile["secret"].as_str().unwrap();
    let before: Value = client
        .get(format!("{}/v1/models", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let before_ids = before["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<HashSet<_>>();
    assert!(before_ids.contains("gpt-test"));
    assert!(!before_ids.contains("gpt-account-only"));

    let runtime = server.state.runtime().unwrap().unwrap();
    assert_eq!(
        client
            .patch(format!("{}/accounts/{account_id}", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"draining": false}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));

    let after: Value = client
        .get(format!("{}/v1/models", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let after_ids = after["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<HashSet<_>>();
    assert!(after_ids.contains("gpt-account-only"));
    assert_eq!(
        client
            .post(format!("{}/v1/responses", server.origin))
            .bearer_auth(pool_key)
            .json(&json!({
                "model": "gpt-account-only",
                "input": "scope refresh",
                "stream": true
            }))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // Pool membership changes the account candidate itself as well as the
    // internal key scope. Re-adding an active account must not require a
    // runtime rebuild: its OAuth executor can continue serving any existing
    // stream while new requests see the candidate immediately.
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"accountIds": [account_id.as_str()], "inPool": false}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));
    let removed: Value = client
        .get(format!("{}/v1/models", server.origin))
        .bearer_auth(pool_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!removed["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["id"] == "gpt-account-only"));

    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"accountIds": [account_id.as_str()], "inPool": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));
    assert_eq!(
        client
            .post(format!("{}/v1/responses", server.origin))
            .bearer_auth(pool_key)
            .json(&json!({
                "model": "gpt-account-only",
                "input": "membership hot apply",
                "stream": true
            }))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    server.task.abort();
    upstream_task.abort();
}

use super::*;

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
    // Unknown prefixed identities follow company order in the snapshot:
    // OpenAI, then Anthropic. Discovery order and route bindings stay stored.
    assert_eq!(source["models"], json!(["gpt-native", "claude-native"]));
    let stored_source = server
        .state
        .store
        .sources()
        .unwrap()
        .into_iter()
        .find(|record| record.id == source["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(stored_source.models, ["gpt-native", "claude-native"]);
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
    // Unknown prefixed identities follow company order in the snapshot:
    // OpenAI, then Anthropic. Discovery order and route bindings stay stored.
    assert_eq!(source["models"], json!(["gpt-native", "claude-native"]));
    let stored_source = server
        .state
        .store
        .sources()
        .unwrap()
        .into_iter()
        .find(|record| record.id == source["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(stored_source.models, ["gpt-native", "claude-native"]);
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
async fn explicit_messages_routes_join_the_shared_pool_for_both_client_protocols() {
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
            "protocolBindings": [
                {
                    "wireApi": "messages",
                    "adapter": "native",
                    "modelIds": ["claude-native"]
                },
                {
                    "wireApi": "responses",
                    "adapter": "responses_to_messages",
                    "modelIds": ["claude-native"]
                }
            ],
            "models": ["claude-native"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let messages_source_id = messages_source["id"].as_str().unwrap();

    let membership = client
        .post(format!("{}/pool/members", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sourceIds": [messages_source_id],
            "inPool": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);

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
    assert!(system_model_ids.contains(&"claude-native"));

    // Source discovery is allowed during setup. Count only the two explicitly
    // configured execution paths below: native Messages and its Responses adapter.
    messages_state.lock().unwrap().clear();

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
    let native = client
        .post(format!("{}/v1/messages", server.origin))
        .header("x-api-key", system_secret)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(native.status(), StatusCode::OK);
    let native: Value = native.json().await.unwrap();
    assert_eq!(native["content"][0]["type"], "tool_use");

    let bridged = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(system_secret)
        .json(&json!({
            "model": "claude-native",
            "input": "use the tool",
            "tools": [{
                "type": "function",
                "name": "read_file",
                "description": "Read a file",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bridged.status(), StatusCode::OK);
    let bridged: Value = bridged.json().await.unwrap();
    assert_eq!(bridged["output"][0]["type"], "function_call");
    assert_eq!(bridged["output"][0]["name"], "read_file");

    let observed = messages_state.lock().unwrap().clone();
    assert_eq!(observed.len(), 2);

    server.task.abort();
    responses_task.abort();
    messages_task.abort();
}

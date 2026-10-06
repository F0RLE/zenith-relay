use super::*;

#[tokio::test]
async fn invalid_foreign_reasoning_retries_a_native_account_without_ciphertext() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"message":"Encrypted content for item rs_1 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}}),
        ),
        success_reply("recovered-response"),
    ])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [
                {"id":"rs_1","type":"reasoning","encrypted_content":"foreign-ciphertext","summary":[{"type":"summary_text","text":"visible old reasoning"}]},
                {"role":"assistant","content":"previous answer"},
                {"role":"user","content":"continue"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body["input"][0]["encrypted_content"],
        "foreign-ciphertext"
    );
    assert_eq!(requests[0].body["input"][0]["id"], "rs_1");
    let retry_input = requests[1].body["input"].as_array().unwrap();
    assert_eq!(retry_input.len(), 3);
    assert_eq!(retry_input[0]["type"], "reasoning");
    assert!(retry_input[0].get("encrypted_content").is_none());
    assert!(retry_input[0].get("id").is_none());
    assert_eq!(
        retry_input[0]["summary"][0]["text"],
        "visible old reasoning"
    );
    assert!(retry_input
        .iter()
        .all(|item| item.get("encrypted_content").is_none()));
    assert!(requests[1].body.to_string().contains("previous answer"));
    assert!(requests[1].body.to_string().contains("continue"));
    drop(requests);
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_foreign_reasoning_repair_stays_on_the_account_in_manual_rotation() {
    let (account_upstream, account_state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"message":"Encrypted content for item rs_1 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}}),
        ),
        success_reply("recovered-response"),
    ])
    .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("source-must-not-run")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source(
            "fallback-source",
            &source_upstream,
            "source-key",
            100,
        )],
        vec![account(
            "relay-account",
            "provider-account",
            &account_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["relay-account", "fallback-source"]);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [
                {"id":"rs_1","type":"reasoning","encrypted_content":"foreign-ciphertext","summary":[{"type":"summary_text","text":"visible old reasoning"}]},
                {"role":"user","content":"continue"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(account_state.requests.lock().unwrap().len(), 2);
    assert!(source_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_foreign_compaction_retries_without_the_rejected_items() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"code":"invalid_encrypted_content"}}),
        ),
        success_reply("recovered-compaction-response"),
    ])
    .await;
    let authority = ready_authority("relay-compaction-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "relay-compaction-account",
            "provider-compaction-account",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [
                {"id":"cmp_stale","type":"compaction","encrypted_content":"invalid"},
                {"id":"cmp_summary_stale","type":"compaction_summary","encrypted_content":"invalid"},
                {"id":"cmp_plain","type":"compaction","summary":[]},
                {"role":"user","content":"continue"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body["input"][0]["encrypted_content"], "invalid");
    assert_eq!(requests[0].body["input"][1]["encrypted_content"], "invalid");
    assert_eq!(requests[0].body["input"].as_array().unwrap().len(), 4);
    let retry_input = requests[1].body["input"].as_array().unwrap();
    assert_eq!(retry_input.len(), 2);
    assert_eq!(retry_input[0]["id"], "cmp_plain");
    assert_eq!(retry_input[1]["role"], "user");
    assert!(retry_input
        .iter()
        .all(|item| item.get("encrypted_content").is_none()));
}

#[tokio::test]
async fn invalid_foreign_compaction_repair_stays_on_the_account_in_manual_rotation() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"message":"Encrypted content for item cmp_1 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}}),
        ),
        Reply::Json(
            StatusCode::OK,
            json!({"type":"compaction","items":[],"usage":{"input_tokens":1}}),
        ),
    ])
    .await;
    let (other_upstream, other_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::OK,
        json!({"type":"compaction","items":[],"usage":{"input_tokens":1}}),
    )])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-compaction-account", "owner-access").await;
    register_ready(&authority, "other-compaction-account", "other-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "owner-compaction-account",
                "provider-owner-compaction",
                &owner_upstream,
                10,
            ),
            account(
                "other-compaction-account",
                "provider-other-compaction",
                &other_upstream,
                10,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(
        &gateway,
        &["owner-compaction-account", "other-compaction-account"],
    );

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [
                {"id":"cmp_1","type":"compaction","encrypted_content":"foreign-ciphertext"},
                {"role":"user","content":"continue"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(owner_state.requests.lock().unwrap().len(), 2);
    assert!(other_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compact_account_repairs_legacy_call_ids_after_strict_rejection() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {"message": "Missing required field: call_id"}}),
        ),
        Reply::Json(
            StatusCode::OK,
            json!({"type": "compaction", "items": [], "usage": {"input_tokens": 2}}),
        ),
    ])
    .await;
    let authority = ready_authority("relay-legacy-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "relay-legacy-account",
            "provider-legacy-account",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [
                {"type": "function_call", "name": "lookup", "arguments": "{}"},
                {"type": "function_call_output", "output": "result"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].body["input"][0].get("call_id").is_none());
    assert_eq!(
        requests[1].body["input"][0]["call_id"],
        requests[1].body["input"][1]["call_id"]
    );
    drop(requests);
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn image_generation_uses_cheapest_account_model_and_translates_response() {
    let (upstream, state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"aW1hZ2U=\",\"output_format\":\"png\"}}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"created_at\":7,\"output\":[],\"tool_usage\":{\"image_gen\":{\"image_tokens\":4}}}}\n\n",
        ),
    ])])
    .await;
    let authority = ready_authority("relay-image-account", "image-access").await;
    let mut image_account = account(
        "relay-image-account",
        "provider-image-account",
        &upstream,
        10,
    );
    image_account.models.push("gpt-5.6-terra".to_string());
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![image_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-image-2.5-sunburst","prompt":"draw a test"}))
        .send()
        .await
        .unwrap();
    let response_status = response.status();
    let response_body = response.text().await.unwrap();
    assert_eq!(response_status, StatusCode::OK, "{response_body}");
    let body: Value = serde_json::from_str(&response_body).unwrap();
    assert_eq!(body["data"][0]["b64_json"], "aW1hZ2U=");

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[0].body["model"], "gpt-5.6-terra");
    assert_eq!(requests[0].body["tools"][0]["type"], "image_generation");
    assert_eq!(
        requests[0].body["tools"][0]["model"],
        "gpt-image-2.5-sunburst"
    );
    assert!(requests[0].body["tools"][0].get("size").is_none());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(
        events[0].requested_model.as_deref(),
        Some("gpt-image-2.5-sunburst")
    );
    assert_eq!(events[0].resolved_model.as_deref(), Some("gpt-5.6-terra"));
}

#[tokio::test]
async fn bounded_image_retry_does_not_report_an_untried_account_as_cooled() {
    let (limited_upstream, limited_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"code": "rate_limit_exceeded"}}),
    )])
    .await;
    let (ready_upstream, ready_state) = spawn_upstream(Vec::new()).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "limited-image", "limited-image-access").await;
    register_ready(&authority, "ready-image", "ready-image-access").await;
    let mut limited_account = account("limited-image", "provider-limited", &limited_upstream, 200);
    limited_account.models.push("gpt-5.6-terra".to_string());
    let mut ready_account = account("ready-image", "provider-ready", &ready_upstream, 100);
    ready_account.models.push("gpt-5.6-terra".to_string());
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![limited_account, ready_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-image-2.5-sunburst","prompt":"draw a test"}))
        .send()
        .await
        .unwrap();
    let response_status = response.status();
    let response_body = response.text().await.unwrap();
    assert_eq!(
        response_status,
        StatusCode::TOO_MANY_REQUESTS,
        "{response_body}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response_body).unwrap()["error"]["code"],
        "rate_limit_exceeded"
    );
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert!(ready_state.requests.lock().unwrap().is_empty());
}

use super::*;

#[tokio::test]
async fn targeted_encrypted_reasoning_repair_preserves_other_items() {
    for diagnostic in [
        json!({"param":"input[1].encrypted_content"}),
        json!({"message":"The encrypted content BBBB...2222 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}),
    ] {
        let mut error = diagnostic;
        error["code"] = json!("invalid_encrypted_content");
        let (upstream, state) = spawn_upstream(vec![
            Reply::Json(StatusCode::BAD_REQUEST, json!({"error":error})),
            success_reply("synthetic-targeted-recovery"),
        ])
        .await;
        let authority = ready_authority("relay-account", "account-access").await;
        let (gateway, _, _, _) = spawn_mixed_gateway(
            Vec::new(),
            vec![account("relay-account", "provider-account", &upstream, 10)],
            vec![mixed_key(None, None)],
            authority,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let input = json!([
            {"id":"rs_first","type":"reasoning","encrypted_content":"AAAA-synthetic-1111","summary":[]},
            {"id":"rs_second","type":"reasoning","encrypted_content":"BBBB-synthetic-2222","summary":[{"type":"summary_text","text":"Visible synthetic summary"}]},
            {"type":"function_call","name":"lookup","call_id":"call_synthetic","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_synthetic","output":"Synthetic result"},
            {"role":"user","content":"Continue synthetic task"}
        ]);
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL,"input":input}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body["input"], input);
        let retry = &requests[1].body["input"];
        assert_eq!(retry[0], input[0]);
        assert_eq!(retry[1]["summary"], input[1]["summary"]);
        assert!(retry[1].get("encrypted_content").is_none());
        assert!(retry[1].get("id").is_none());
        for index in 2..5 {
            assert_eq!(retry[index], input[index]);
        }
    }
}

#[tokio::test]
async fn invalid_foreign_reasoning_retries_a_native_account_without_ciphertext() {
    assert_foreign_reasoning_repair(false).await;
}

#[tokio::test]
async fn invalid_foreign_reasoning_with_generated_usage_does_not_repeat_the_turn() {
    assert_foreign_reasoning_repair(true).await;
}

async fn assert_foreign_reasoning_repair(generated: bool) {
    let mut failure = json!({"error":{"message":"Encrypted content for item rs_1 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}});
    if generated {
        failure["usage"] = json!({"input_tokens":5,"output_tokens":7});
    }
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(StatusCode::BAD_REQUEST, failure),
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
    assert_eq!(
        response.status(),
        if generated {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::OK
        }
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), if generated { 1 } else { 2 });
    assert_eq!(
        requests[0].body["input"][0]["encrypted_content"],
        "foreign-ciphertext"
    );
    assert_eq!(requests[0].body["input"][0]["id"], "rs_1");
    if generated {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].output_tokens, Some(7));
        assert!(!events[0].success);
        return;
    }
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
async fn invalid_foreign_compaction_preserves_history_without_retrying() {
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["encrypted_content"], "invalid");
    assert_eq!(requests[0].body["input"][1]["encrypted_content"], "invalid");
    assert_eq!(requests[0].body["input"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn invalid_foreign_compaction_does_not_retry_or_rotate_in_manual_order() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"message":"Encrypted content for item cmp_1 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}}),
        ),
        Reply::Json(
            StatusCode::OK,
            compaction_body("owner-compaction"),
        ),
    ])
    .await;
    let (other_upstream, other_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::OK,
        compaction_body("other-compaction"),
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
    assert!(other_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compact_account_repairs_legacy_call_ids_after_strict_rejection() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {"message": "Missing required field: call_id"}}),
        ),
        Reply::Json(StatusCode::OK, compaction_body("repaired-compaction")),
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
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0]
            .routing
            .as_ref()
            .and_then(|routing| routing.attempt_outcome),
        Some(zenith_relay_core::AttemptOutcome::RetryRepair)
    );
    assert!(events[1].success);
}

#[tokio::test]
async fn image_generation_uses_native_account_endpoint_without_a_text_model() {
    let (upstream, state) = spawn_upstream(vec![Reply::Json(
        StatusCode::OK,
        json!({
            "created":7, "data":[{"b64_json":"aW1hZ2U="}],
            "usage":{"input_tokens":3,"output_tokens":4,"total_tokens":7,
                "input_tokens_details":{"text_tokens":3,"image_tokens":0}}
        }),
    )])
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
    assert_eq!(requests[0].path, "/v1/images/generations");
    assert_eq!(requests[0].body["model"], "gpt-image-2.5-sunburst");
    assert!(requests[0].body.get("tools").is_none());
    assert!(requests[0].body.get("reasoning").is_none());
    assert!(requests[0].body.get("size").is_none());
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer image-access")
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(
        events[0].requested_model.as_deref(),
        Some("gpt-image-2.5-sunburst")
    );
    assert_eq!(
        events[0].resolved_model.as_deref(),
        Some("gpt-image-2.5-sunburst")
    );
    assert_eq!(events[0].total_tokens, Some(7));
    assert_eq!(
        events[0].image_usage.as_ref().unwrap().input_text_tokens,
        Some(3)
    );
    assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 1);
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

#[tokio::test]
async fn native_image_stream_and_incomplete_json_keep_usage_without_replay() {
    for (stream, reply) in [
        (true, Reply::Stream(vec![
            StreamChunk::Data("data: {\"type\":\"image_generation.partial_image\",\"b64_json\":\"cHJldmlldw==\"}\n\n"),
            StreamChunk::Data("data: {\"type\":\"image_generation.completed\",\"b64_json\":\"aW1hZ2U=\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3,\"total_tokens\":5}}\n\n"),
        ])),
        (false, Reply::Json(StatusCode::OK, json!({"data":[],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}))),
    ] {
        let (upstream, state) = spawn_upstream(vec![reply]).await;
        let authority = ready_authority("image-account", "synthetic-access").await;
        let (gateway, events, _, _) = spawn_mixed_gateway(Vec::new(),
            vec![account("image-account", "synthetic-provider", &upstream, 10)],
            vec![mixed_key(None, None)], authority, refresh_adapter(), Arc::new(PersistenceAdapter::default())).await;
        let response = reqwest::Client::new().post(format!("{}/v1/images/generations", gateway.base_url))
            .bearer_auth(LOCAL_KEY).json(&json!({"model":"gpt-image-2.5-sunburst","prompt":"synthetic","stream":stream}))
            .send().await.unwrap();
        assert_eq!(response.status(), if stream { StatusCode::OK } else { StatusCode::BAD_GATEWAY });
        let body = response.text().await.unwrap();
        if stream { assert!(body.contains("image_generation.partial_image")); assert!(body.contains("image_generation.completed")); }
        assert_eq!(state.requests.lock().unwrap().len(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].success, stream);
        assert_eq!(events[0].total_tokens, Some(5));
        if !stream { assert_eq!(events[0].error_category.as_deref(), Some("image_output_missing")); }
    }
}

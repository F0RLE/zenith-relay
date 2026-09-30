use super::*;

#[tokio::test]
async fn unknown_http_response_owner_is_rejected_before_candidate_selection() {
    let (wrong_upstream, wrong_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Previous response with id 'response-from-before-restart' not found.",
            "type": "invalid_request_error",
            "code": "previous_response_not_found"
        }}),
    )])
    .await;
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        success_reply("recovered-response"),
        success_reply("continued-response"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "wrong-account", "wrong-access").await;
    register_ready(&authority, "owner-account", "owner-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("wrong-account", "provider-wrong", &wrong_upstream, 100),
            account("owner-account", "provider-owner", &owner_upstream, 10),
        ],
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
            "input": "continue after restart",
            "previous_response_id": "response-from-before-restart"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(wrong_state.requests.lock().unwrap().is_empty());
    assert!(owner_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn unknown_http_response_owner_never_reaches_an_arbitrary_candidate() {
    let (wrong_upstream, wrong_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Invalid request body.",
            "type": "invalid_request_error",
            "code": "invalid_request"
        }}),
    )])
    .await;
    let (owner_upstream, owner_state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "wrong-account", "wrong-access").await;
    register_ready(&authority, "owner-account", "owner-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("wrong-account", "provider-wrong", &wrong_upstream, 100),
            account("owner-account", "provider-owner", &owner_upstream, 10),
        ],
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
            "input": "invalid continuation",
            "previous_response_id": "response-from-before-restart"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(wrong_state.requests.lock().unwrap().is_empty());
    assert!(owner_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn orphaned_http_response_is_rejected_without_materialized_history() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {
                "message": "Previous response with id 'orphaned-response' not found.",
                "type": "invalid_request_error",
                "code": "previous_response_not_found"
            }}),
        ),
        success_reply("fresh-response"),
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
            "input": "continue without an available response owner",
            "previous_response_id": "orphaned-response"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn opaque_http_continuation_is_rejected_before_model_switch() {
    let (old_model_upstream, old_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Tool call output does not match the model that created the previous response",
            "type": "invalid_request_error",
            "code": "tool_call_not_found"
        }}),
    )])
    .await;
    let (new_model_upstream, new_state) =
        spawn_upstream(vec![success_reply("switched-model-response")]).await;
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    register_ready(&authority, "old-model-account", "old-model-access").await;
    register_ready(&authority, "new-model-account", "new-model-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "old-model-account",
                "provider-old",
                &old_model_upstream,
                100,
            ),
            account("new-model-account", "provider-new", &new_model_upstream, 10),
        ],
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
            "input": "continue after switching models",
            "previous_response_id": "response-created-by-old-model"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(old_state.requests.lock().unwrap().is_empty());
    assert!(new_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn opaque_continuation_is_rejected_for_regular_responses() {
    assert_opaque_continuation_is_rejected("/v1/responses").await;
}

#[tokio::test]
async fn opaque_continuation_is_rejected_for_compact_responses() {
    assert_opaque_continuation_is_rejected("/v1/responses/compact").await;
}

async fn assert_opaque_continuation_is_rejected(path: &str) {
    let (old_model_upstream, old_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Tool call output does not match the model that created the previous response",
            "type": "invalid_request_error",
            "code": "tool_call_not_found"
        }}),
    )])
    .await;
    let (rejected_upstream, rejected_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {"message": "this route cannot satisfy the request"}}),
    )])
    .await;
    let (recovered_upstream, recovered_state) =
        spawn_upstream(vec![success_reply("repaired-continuation")]).await;
    let authority = Arc::new(TokenAuthority::new(3).unwrap());
    register_ready(&authority, "old-model-account", "old-model-access").await;
    register_ready(
        &authority,
        "rejected-model-account",
        "rejected-model-access",
    )
    .await;
    register_ready(
        &authority,
        "recovered-model-account",
        "recovered-model-access",
    )
    .await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "old-model-account",
                "provider-old",
                &old_model_upstream,
                300,
            ),
            account(
                "rejected-model-account",
                "provider-rejected",
                &rejected_upstream,
                200,
            ),
            account(
                "recovered-model-account",
                "provider-recovered",
                &recovered_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}{path}", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue after switching models",
            "previous_response_id": "response-created-by-old-model"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(old_state.requests.lock().unwrap().is_empty());
    assert!(rejected_state.requests.lock().unwrap().is_empty());
    assert!(recovered_state.requests.lock().unwrap().is_empty());

    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn opaque_custom_tool_history_is_rejected_before_upstream_selection() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {
                "message": "No tool output found for custom tool call ctc_stale_tool",
                "type": "invalid_request_error",
                "code": "tool_call_not_found"
            }}),
        ),
        success_reply("switched-model-response"),
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
            "previous_response_id": "response-created-by-previous-model",
            "input": [
                {
                    "type": "custom_tool_call",
                    "id": "ctc_stale_tool",
                    "call_id": "toolu_stale_tool",
                    "name": "PowerShell",
                    "input": "Get-ChildItem"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Continue with the new model."}]
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(state.requests.lock().unwrap().is_empty());

    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn orphaned_http_tool_output_is_rejected_before_upstream_selection() {
    let (upstream, state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Previous response with id 'orphaned-response' not found.",
            "type": "invalid_request_error",
            "code": "previous_response_not_found"
        }}),
    )])
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
            "input": [{
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "done"
            }],
            "previous_response_id": "orphaned-response"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(state.requests.lock().unwrap().is_empty());
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compact_missing_owned_response_never_sends_opaque_id_to_another_account() {
    for saved_history in [false, true] {
        let mut first_reply = success_reply("compact-owner");
        if !saved_history {
            if let Reply::Json(_, body) = &mut first_reply {
                body.as_object_mut().unwrap().remove("output");
            }
        }
        let (owner, owner_state) = spawn_upstream(vec![
            first_reply,
            Reply::Json(
                StatusCode::BAD_REQUEST,
                json!({"error":{"code":"previous_response_not_found"}}),
            ),
            success_reply("compact-recovered"),
        ])
        .await;
        let (backup, backup_state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
        let authority = Arc::new(TokenAuthority::new(2).unwrap());
        register_ready(&authority, "owner", "owner-access").await;
        register_ready(&authority, "backup", "backup-access").await;
        let (gateway, _, _, _) = spawn_mixed_gateway(
            Vec::new(),
            vec![
                account("owner", "owner-provider", &owner, 100),
                account("backup", "backup-provider", &backup, 10),
            ],
            vec![mixed_key(None, None)],
            authority,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        rotation_policy::set_order(&gateway, &["owner", "backup"]);
        let client = reqwest::Client::new();
        let first = request(&gateway, false).await;
        assert_eq!(first.status(), StatusCode::OK);
        let first: Value = first.json().await.unwrap();
        let input = json!([{"role":"assistant","content":"synthetic answer"},
            {"role":"user","content":"continue"}]);
        let response = client
            .post(format!("{}/v1/responses/compact", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL,"previous_response_id":first["id"],"input":input}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if saved_history {
                StatusCode::OK
            } else {
                StatusCode::CONFLICT
            }
        );
        let response: Value = response.json().await.unwrap();
        assert!(backup_state.requests.lock().unwrap().is_empty());
        let requests = owner_state.requests.lock().unwrap();
        assert_eq!(requests.len(), if saved_history { 3 } else { 2 });
        if saved_history {
            assert!(requests[2].body.get("previous_response_id").is_none());
            assert_eq!(requests[2].body["input"][0]["content"][0]["text"], "hello");
            assert_eq!(
                requests[2].body["input"].as_array().unwrap().last(),
                input.as_array().unwrap().last()
            );
        } else {
            assert_eq!(
                response["error"]["code"],
                "response_continuation_unavailable"
            );
        }
    }
}

#[tokio::test]
async fn stale_http_response_affinity_resets_before_quota_routing() {
    assert_stale_http_continuation(false).await;
}

#[tokio::test]
async fn stale_http_sse_response_replays_before_output() {
    assert_stale_http_continuation(true).await;
}

async fn assert_stale_http_continuation(sse_failure: bool) {
    let (fallback_upstream, fallback_state) =
        spawn_upstream(vec![success_reply("fallback-response")]).await;
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        success_reply("stale-response"),
        if sse_failure {
            Reply::Stream(vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"previous_response_not_found\",\"message\":\"Previous response not found\"}}}\n\n",
            )])
        } else { Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {
                "message": "Previous response with id 'stale-response' not found.",
                "type": "invalid_request_error",
                "code": "previous_response_not_found"
            }}),
        ) },
        success_reply("recovered-response"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "fallback-account", "fallback-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account(
                "fallback-account",
                "provider-fallback",
                &fallback_upstream,
                10,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["owner-account", "fallback-account"]);

    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "start"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "stale-response");

    let recovered: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue after stale binding",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["id"], "recovered-response");
    let requests = owner_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].body.get("previous_response_id").is_none());
    assert_eq!(requests[2].body["input"][0]["content"][0]["text"], "start");
    assert_eq!(
        requests[2].body["input"][1]["content"][0]["text"],
        "continue after stale binding"
    );
    drop(requests);
    assert!(fallback_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[0].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert!(events[2].success);
    assert_eq!(events[2].candidate_id.as_deref(), Some("owner-account"));
}

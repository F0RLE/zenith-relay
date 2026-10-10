use super::*;
use zenith_relay_core::usage::{
    CacheContextBaseline, CacheContextScope, CacheContextSection, CacheHistoryComparison,
    CacheInputKind,
};

async fn capture_native_cache_request(
    State(observed): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Response<Body> {
    observed.lock().unwrap().push(body.clone());
    let mut response = json!({
        "id": "resp_synthetic_cache",
        "object": "response",
        "status": "completed",
        "model": "gpt-cache-test",
        "output": [{
            "id": "msg_synthetic_cache",
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": "synthetic reply"}]
        }],
        "usage": {
            "input_tokens": 164_283,
            "input_tokens_details": {
                "cached_tokens": 138_971,
                "cache_write_tokens": 0
            },
            "output_tokens": 515,
            "total_tokens": 164_798
        }
    });
    if body["input"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["type"] == "compaction_trigger")
    }) {
        response["output"] = json!([{
            "type": "compaction",
            "encrypted_content": "synthetic-compaction-result"
        }]);
    }
    if body["stream"] == true {
        Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "event: response.completed\ndata: {}\n\n",
                json!({"type": "response.completed", "response": response})
            )))
            .unwrap()
    } else {
        Json(response).into_response()
    }
}

/// A native API source must receive the client's context, not a second copy
/// of Relay's saved history. Cache controls and tool order are client-owned
/// even when HTTP/SSE continuations have local replay state available.
#[tokio::test]
async fn native_responses_preserves_cache_prefix_and_does_not_duplicate_history() {
    for stream in [false, true] {
        let observed = Arc::new(Mutex::new(Vec::<Value>::new()));
        let upstream = spawn(
            Router::new()
                .fallback(post(capture_native_cache_request))
                .with_state(observed.clone()),
        )
        .await;
        let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-cache-test"]).await;

        let tools = (0..90)
            .map(|index| {
                json!({
                    "type": "function",
                    "name": format!("synthetic_tool_{index}"),
                    "description": "Synthetic stable tool description. ".repeat(32),
                    "parameters": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}},
                        "required": ["value"],
                        "additionalProperties": false
                    }
                })
            })
            .collect::<Vec<_>>();
        let first = json!({
            "model": "gpt-cache-test",
            "stream": stream,
            "store": false,
            "instructions": "Synthetic stable instructions.",
            "reasoning": {"effort": "high", "context": "all_turns"},
            "text": {"verbosity": "medium"},
            "parallel_tool_calls": true,
            "prompt_cache_key": "synthetic-stable-cache",
            "prompt_cache_retention": "24h",
            "prompt_cache_options": {"mode": "implicit", "ttl": "30m"},
            "tools": tools,
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{
                        "type": "input_text",
                        "text": "Synthetic stable context. ".repeat(1_024),
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    }]
                },
                {
                    "type": "reasoning",
                    "id": "rs_synthetic_history",
                    "summary": [],
                    "encrypted_content": "synthetic-encrypted-history"
                },
                {"role": "user", "content": "Synthetic first turn."}
            ]
        });
        let delta = json!([
            {"type": "configuration_update", "reasoning": {"effort": "max"}},
            {"role": "developer", "content": "Synthetic appended instructions."},
            {
                "type": "additional_tools",
                "role": "developer",
                "tools": [{
                    "type": "function",
                    "name": "synthetic_loaded_tool",
                    "parameters": {"type": "object", "properties": {}}
                }]
            },
            {"role": "user", "content": "Synthetic next turn."}
        ]);
        let mut continuation = first.clone();
        continuation["previous_response_id"] = json!("resp_synthetic_cache");
        continuation["prompt_cache_options"]["comparison_response_id"] =
            json!("resp_synthetic_cache");
        continuation["input"] = delta.clone();
        let mut complete_history = first.clone();
        let history = complete_history["input"].as_array_mut().unwrap();
        history.push(json!({
            "id": "msg_synthetic_cache",
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": "synthetic reply"}]
        }));
        history.extend(delta.as_array().unwrap().iter().cloned());

        let expected = vec![first, continuation, complete_history];
        let client = reqwest::Client::new();
        for request in &expected {
            let response = client
                .post(format!("{}/v1/responses", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .header("x-codex-session-id", "synthetic-stable-session")
                .json(request)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "stream={stream}");
            assert!(response
                .text()
                .await
                .unwrap()
                .contains("resp_synthetic_cache"));
        }
        assert_eq!(
            *observed.lock().unwrap(),
            expected,
            "native context, tool order and cache controls must stay unchanged"
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            while events.lock().unwrap().len() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3);
        for event in events.iter() {
            assert_eq!(event.input_tokens, Some(164_283));
            assert_eq!(event.cached_input_tokens, Some(138_971));
            assert_eq!(event.cache_write_input_tokens, Some(0));
            assert_eq!(event.output_tokens, Some(515));
            assert_eq!(event.total_tokens, Some(164_798));
            let diagnostics = event
                .routing
                .as_ref()
                .unwrap()
                .cache_context
                .as_ref()
                .unwrap();
            assert_eq!(diagnostics.scope, CacheContextScope::ClientSession);
            assert!(diagnostics.relay_changes.is_empty());
            assert_eq!(
                diagnostics.relay_history.comparison,
                CacheHistoryComparison::Unchanged
            );
        }
        let first = events[0]
            .routing
            .as_ref()
            .unwrap()
            .cache_context
            .as_ref()
            .unwrap();
        let continuation = events[1]
            .routing
            .as_ref()
            .unwrap()
            .cache_context
            .as_ref()
            .unwrap();
        assert_eq!(first.baseline, CacheContextBaseline::FirstObservation);
        assert_eq!(
            continuation.baseline,
            CacheContextBaseline::CompletedRequest
        );
        assert_eq!(
            continuation.client_history.comparison,
            CacheHistoryComparison::Continuation
        );
        assert_eq!(
            continuation.upstream_history.comparison,
            CacheHistoryComparison::Continuation
        );
        assert_eq!(continuation.candidate_changed, Some(false));
    }
}

#[tokio::test]
async fn native_responses_reports_client_prefix_changes_without_blame_on_relay() {
    let observed = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = spawn(
        Router::new()
            .fallback(post(capture_native_cache_request))
            .with_state(observed.clone()),
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-cache-test"]).await;
    let first = json!({
        "model": "gpt-cache-test",
        "instructions": "Synthetic stable instructions.",
        "reasoning": {"effort": "high"},
        "tools": [
            {"type": "function", "name": "synthetic_a", "parameters": {"type": "object"}},
            {"type": "function", "name": "synthetic_b", "parameters": {"type": "object"}}
        ],
        "input": [
            {"role": "developer", "content": "Synthetic initial prefix."},
            {"role": "user", "content": "Synthetic question."}
        ]
    });
    let mut next = first.clone();
    next["tools"].as_array_mut().unwrap().reverse();
    next["reasoning"]["effort"] = json!("low");
    next["input"][0]["content"] = json!("Synthetic rewritten prefix.");
    let client = reqwest::Client::new();
    for request in [&first, &next] {
        let response = client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .header("x-codex-session-id", "synthetic-comparison-session")
            .json(request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.bytes().await.unwrap();
    }
    assert_eq!(*observed.lock().unwrap(), vec![first, next]);
    let events = events.lock().unwrap();
    let diagnostics = events[1]
        .routing
        .as_ref()
        .unwrap()
        .cache_context
        .as_ref()
        .unwrap();
    assert_eq!(diagnostics.baseline, CacheContextBaseline::CompletedRequest);
    assert_eq!(
        diagnostics.client_changes,
        vec![CacheContextSection::Tools, CacheContextSection::Reasoning]
    );
    assert_eq!(diagnostics.upstream_changes, diagnostics.client_changes);
    assert!(diagnostics.relay_changes.is_empty());
    assert_eq!(
        diagnostics.client_history.comparison,
        CacheHistoryComparison::Rewritten
    );
    assert_eq!(diagnostics.client_history.shared_prefix_items, Some(0));
    assert_eq!(
        diagnostics.client_history.first_changed_item_kind,
        Some(CacheInputKind::Developer)
    );
    assert_eq!(
        diagnostics.relay_history.comparison,
        CacheHistoryComparison::Unchanged
    );
}

#[tokio::test]
async fn native_compaction_does_not_replace_conversation_comparison_baseline() {
    let observed = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = spawn(
        Router::new()
            .fallback(post(capture_native_cache_request))
            .with_state(observed),
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-cache-test"]).await;
    let first = json!({
        "model": "gpt-cache-test",
        "input": [{"role": "user", "content": "Synthetic first turn."}]
    });
    let mut next = first.clone();
    next["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "user", "content": "Synthetic next turn."}));
    let client = reqwest::Client::new();
    for (path, request) in [
        ("/v1/responses", &first),
        ("/v1/responses/compact", &first),
        ("/v1/responses", &next),
    ] {
        let response = client
            .post(format!("{}{path}", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .header("x-codex-session-id", "synthetic-compaction-session")
            .json(request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        response.bytes().await.unwrap();
    }
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[1].routing.as_ref().unwrap().cache_context.is_none());
    let next = events[2]
        .routing
        .as_ref()
        .unwrap()
        .cache_context
        .as_ref()
        .unwrap();
    assert_eq!(next.baseline, CacheContextBaseline::CompletedRequest);
    assert_eq!(
        next.client_history.comparison,
        CacheHistoryComparison::Appended
    );
    assert_eq!(next.client_history.shared_prefix_items, Some(1));
    assert!(next.client_changes.is_empty());
}

#[tokio::test]
async fn bridged_responses_does_not_claim_a_native_context_comparison() {
    let upstream = spawn(Router::new().fallback(post(|| async {
        Json(json!({
            "id": "chat_synthetic",
            "object": "chat.completion",
            "model": "synthetic-chat",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Synthetic reply."}, "finish_reason": "stop"}]
        }))
    })))
    .await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::new(
        ProviderSource {
            id: "synthetic-chat-source".into(),
            name: "Synthetic chat".into(),
            base_url: upstream.base_url.clone(),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::ChatCompletions,
            models: vec!["synthetic-chat".into()],
        },
        LocalGatewayKey {
            id: "synthetic-local".into(),
            secret: LOCAL_KEY.into(),
        },
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    let gateway = spawn(gateway::router(Arc::new(runtime))).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-codex-session-id", "synthetic-bridged-session")
        .json(&json!({"model": "synthetic-chat", "input": "Synthetic question."}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].routing.as_ref().unwrap().cache_context.is_none());
}

#[tokio::test]
async fn native_responses_websocket_http_fallback_preserves_cache_controls_and_context() {
    let observed = Arc::new(Mutex::new(Vec::<Value>::new()));
    // This upstream deliberately supports POST/SSE but not WebSocket upgrades.
    let upstream = spawn(
        Router::new()
            .fallback(post(capture_native_cache_request))
            .with_state(observed.clone()),
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-cache-test"]).await;
    let first = json!({
        "model": "gpt-cache-test",
        "stream": true,
        "reasoning": {"effort": "high", "context": "all_turns"},
        "prompt_cache_key": "synthetic-websocket-cache",
        "prompt_cache_options": {"mode": "implicit", "ttl": "30m"},
        "tools": [{
            "type": "function",
            "name": "synthetic_lookup",
            "parameters": {"type": "object", "properties": {}}
        }],
        "input": [
            {
                "role": "developer",
                "content": [{
                    "type": "input_text",
                    "text": "Synthetic stable context. ".repeat(1_024),
                    "prompt_cache_breakpoint": {"mode": "explicit"}
                }]
            },
            {"role": "user", "content": "Synthetic first turn."}
        ]
    });
    let mut continuation = first.clone();
    continuation["previous_response_id"] = json!("resp_synthetic_cache");
    // Disabling calls must not silently remove the cache-stable tool catalog.
    continuation["tool_choice"] = json!("none");
    continuation["input"] = json!([
        {"type": "configuration_update", "reasoning": {"effort": "max"}},
        {"role": "user", "content": "Synthetic next turn."}
    ]);
    let expected = vec![first, continuation];
    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-codex-session-id", "synthetic-websocket-session")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    for request in &expected {
        let mut frame = request.clone();
        frame["type"] = json!("response.create");
        socket
            .send(ClientWsMessage::Text(frame.to_string()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let frame = socket.next().await.unwrap().unwrap();
                let ClientWsMessage::Text(text) = frame else {
                    continue;
                };
                let payload: Value = serde_json::from_str(text.as_ref()).unwrap();
                assert_ne!(payload["type"], "error", "{payload}");
                if payload["type"] == "response.completed" {
                    assert_eq!(payload["response"]["usage"]["input_tokens"], 164_283);
                    assert_eq!(
                        payload["response"]["usage"]["input_tokens_details"]["cached_tokens"],
                        138_971
                    );
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(
        *observed.lock().unwrap(),
        expected,
        "transport fallback may strip its envelope, not cache settings or history"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.success)
            .count()
            < 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let events = events.lock().unwrap();
    let completed = events
        .iter()
        .filter(|event| event.success)
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 2);
    for event in &completed {
        assert_eq!(
            event.transport,
            zenith_relay_core::UsageTransport::Websocket
        );
        assert_eq!(event.input_tokens, Some(164_283));
        assert_eq!(event.cached_input_tokens, Some(138_971));
        let diagnostics = event
            .routing
            .as_ref()
            .unwrap()
            .cache_context
            .as_ref()
            .unwrap();
        assert!(diagnostics.relay_changes.is_empty());
        assert_eq!(
            diagnostics.relay_history.comparison,
            CacheHistoryComparison::Unchanged
        );
    }
    let next = completed[1]
        .routing
        .as_ref()
        .unwrap()
        .cache_context
        .as_ref()
        .unwrap();
    assert_eq!(next.baseline, CacheContextBaseline::CompletedRequest);
    assert_eq!(next.client_changes, vec![CacheContextSection::ToolChoice]);
    assert_eq!(next.upstream_changes, vec![CacheContextSection::ToolChoice]);
}

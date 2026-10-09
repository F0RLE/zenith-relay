use super::*;

#[tokio::test]
async fn native_responses_replays_http_tool_continuation_when_upstream_requires_websocket() {
    let (upstream, state) = spawn_native_replay_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();
    let tools = json!([{
        "type": "function",
        "name": "run_command",
        "parameters": {
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        }
    }]);

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": tools
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();
    assert_eq!(first["id"], "resp_native_tool");
    assert_eq!(first["output"][0]["type"], "function_call");

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second: Value = second.json().await.unwrap();
    assert_eq!(
        second["output"][0]["content"][0]["text"],
        "Tool result received"
    );

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    let replayed_input = bodies[2]["input"].as_array().unwrap();
    assert_eq!(replayed_input[1]["type"], "function_call");
    assert_eq!(replayed_input[2]["type"], "function_call_output");
    assert_eq!(replayed_input[2]["call_id"], "call_native_tool");
}

#[tokio::test]
async fn native_responses_replays_tool_continuation_after_invalid_call_id_rejection() {
    let (upstream, state) = spawn_native_replay_upstream_with_rejection(
        NativeReplayRejection::InvalidFunctionCallOutputCallId,
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    let replayed_input = bodies[2]["input"].as_array().unwrap();
    assert_eq!(replayed_input[1]["type"], "function_call");
    assert_eq!(replayed_input[2]["type"], "function_call_output");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(!events[1].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
    assert!(events[2].success);
}

#[tokio::test]
async fn native_responses_does_not_replay_tool_continuation_after_generic_bad_request() {
    let (upstream, state) =
        spawn_native_replay_upstream_with_rejection(NativeReplayRejection::GenericInvalidRequest)
            .await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_responses_keeps_tool_history_after_generic_gateway_rejection() {
    let (upstream, state) = spawn_native_replay_upstream_with_rejection(
        NativeReplayRejection::ZenithGatewayInvalidRequest,
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert_eq!(bodies[1]["input"][0]["type"], "function_call_output");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(!events[1].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
}

#[tokio::test]
async fn native_responses_stream_keeps_tool_history_after_generic_gateway_rejection() {
    let (upstream, state) = spawn_native_replay_upstream_with_rejection(
        NativeReplayRejection::ZenithGatewayInvalidRequestStream,
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "stream": true,
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let second = second.text().await.unwrap();
    assert!(second.contains("Zenith AI request is invalid"));
    assert!(!second.contains("response.completed"));
    assert!(!second.contains("resp_rejected"));

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert_eq!(bodies[1]["stream"], true);
    assert_eq!(bodies[1]["input"][0]["type"], "function_call_output");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(!events[1].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
}

#[tokio::test]
async fn native_responses_stream_replays_tool_continuation_when_upstream_requires_websocket() {
    let (upstream, state) = spawn_native_replay_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();
    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "stream": true,
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = first.text().await.unwrap();
    assert!(first.contains("\"type\":\"response.output_item.done\""));
    assert!(first.contains("\"type\":\"response.completed\""));

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "stream": true,
            "previous_response_id": "resp_native_tool",
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second = second.text().await.unwrap();
    assert!(second.contains("\"delta\":\"Tool result received\""));
    assert!(second.contains("\"type\":\"response.completed\""));

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    assert_eq!(bodies[2]["stream"], true);
    let replayed_input = bodies[2]["input"].as_array().unwrap();
    assert_eq!(replayed_input[1]["type"], "function_call");
    assert_eq!(replayed_input[2]["type"], "function_call_output");
}

#[tokio::test]
async fn native_responses_repair_call_prefixed_function_item_ids_after_strict_rejection() {
    let (upstream, state) = spawn_strict_function_item_id_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": [
                {
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Inspect the workspace"}]
                },
                {
                    "type": "function_call",
                    "id": "call_cross_provider_01",
                    "call_id": "call_cross_provider_01",
                    "name": "run_command",
                    "arguments": "{\"command\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_cross_provider_01",
                    "output": "C:\\workspace"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["id"], "resp_strict_function_id");

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["input"][1]["id"], "call_cross_provider_01");
    assert_eq!(bodies[1]["input"][1]["id"], "fc_call_cross_provider_01");
    assert_eq!(bodies[1]["input"][1]["call_id"], "call_cross_provider_01");
    assert_eq!(bodies[1]["input"][2]["call_id"], "call_cross_provider_01");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn native_responses_repair_legacy_call_ids_after_explicit_strict_rejection() {
    let (upstream, state) = spawn_strict_missing_call_id_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": [
                {"type": "function_call", "id": "fc_legacy", "name": "lookup", "namespace": "functions", "arguments": "{}"},
                {"type": "function_call_output", "output": "lookup result"},
                {"type": "function_call_output", "name": "heartbeat", "output": "standalone"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "resp_strict_call_id"
    );

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["input"].as_array().unwrap().len(), 3);
    let repaired = bodies[1]["input"].as_array().unwrap();
    assert_eq!(repaired.len(), 3);
    assert_eq!(repaired[0]["type"], "function_call");
    assert_eq!(repaired[0]["id"], "fc_legacy");
    assert_eq!(repaired[0]["namespace"], "functions");
    assert_eq!(repaired[1]["call_id"], repaired[0]["call_id"]);
    assert_eq!(repaired[2]["name"], "heartbeat");
    assert!(repaired[2].get("call_id").is_none());
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn native_responses_stream_repairs_legacy_call_ids_after_terminal_error() {
    let (upstream, state) = spawn_strict_missing_call_id_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "stream": true,
            "input": [
                {"type": "custom_tool_call", "name": "patch", "input": "{}"},
                {"type": "custom_tool_call_output", "output": "done"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("History accepted"));
    assert!(body.contains("response.completed"));
    assert!(!body.contains("Missing required field"));

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[0]["input"][0].get("call_id").is_none());
    assert_eq!(
        bodies[1]["input"][0]["call_id"],
        bodies[1]["input"][1]["call_id"]
    );
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn native_tool_links_recover_item_ids_without_losing_results_in_json_and_sse() {
    for streaming in [false, true] {
        for kind in ["function_call", "custom_tool_call"] {
            for call_id in [None, Some("call_stable")] {
                let (upstream, state) = spawn_strict_missing_call_id_upstream().await;
                let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
                let mut call = json!({"type":kind,"id":"ctc_item","name":"patch","input":"synthetic","arguments":"{}"});
                if let Some(id) = call_id {
                    call["call_id"] = json!(id);
                }
                let input = json!([call,
                    {"type":format!("{kind}_output"),"call_id":"ctc_item","output":"synthetic result"}
                ]);
                let response = reqwest::Client::new()
                    .post(format!("{}/v1/responses", gateway.base_url))
                    .bearer_auth(LOCAL_KEY)
                    .json(&json!({"model":"gpt-test","stream":streaming,"input":input}))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert!(response.text().await.unwrap().contains("History accepted"));
                let bodies = state.bodies.lock().unwrap();
                assert_eq!(bodies.len(), 2);
                assert_eq!(bodies[0]["input"], input);
                let mut expected = input;
                expected[0]["call_id"] = json!(call_id.unwrap_or("ctc_item"));
                expected[1]["call_id"] = expected[0]["call_id"].clone();
                assert_eq!(bodies[1]["input"], expected);
                assert!(events.lock().unwrap().iter().all(|event| event.success));
            }
        }
    }
}

#[tokio::test]
async fn native_tool_repair_never_drops_an_orphan_result_to_make_a_retry_succeed() {
    let (upstream, state) = spawn_strict_missing_call_id_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let input = json!([
        {"type":"function_call","name":"lookup","arguments":"{}"},
        {"type":"function_call_output","output":"paired"},
        {"type":"custom_tool_call_output","output":"unresolved"}
    ]);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-test","input":input}))
        .send()
        .await
        .unwrap();
    assert!(!response.status().is_success());
    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["input"], input);
}

#[tokio::test]
async fn native_responses_repair_custom_tool_item_ids_after_strict_rejection() {
    let (upstream, state) = spawn_strict_custom_tool_item_id_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": [
                {
                    "type": "custom_tool_call",
                    "id": "fc_cross_provider_custom_01",
                    "call_id": "toolu_cross_provider_custom_01",
                    "name": "PowerShell",
                    "input": "Get-ChildItem"
                },
                {
                    "type": "custom_tool_call_output",
                    "call_id": "toolu_cross_provider_custom_01",
                    "output": "Cargo.toml"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["id"], "resp_strict_custom_id");

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["input"][0]["id"], "fc_cross_provider_custom_01");
    assert_eq!(
        bodies[1]["input"][0]["id"],
        "ctc_fc_cross_provider_custom_01"
    );
    assert_eq!(
        bodies[1]["input"][0]["call_id"],
        "toolu_cross_provider_custom_01"
    );
    assert_eq!(
        bodies[1]["input"][1]["call_id"],
        "toolu_cross_provider_custom_01"
    );
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn native_responses_remove_item_prefixed_message_ids_after_strict_rejection() {
    let (upstream, state) = spawn_strict_message_item_id_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": [
                {
                    "type": "message",
                    "id": "item_foreign_user_01",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Inspect the workspace"}]
                },
                {
                    "id": "item_foreign_assistant_01",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "I will inspect it."}]
                },
                {
                    "type": "message",
                    "id": "msg_native_01",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "Keep changes scoped."}]
                },
                {
                    "type": "function_call",
                    "id": "item_function_01",
                    "call_id": "call_function_01",
                    "name": "run_command",
                    "arguments": "{\"command\":\"pwd\"}"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["id"], "resp_strict_message_id");

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["input"][0]["id"], "item_foreign_user_01");
    assert_eq!(bodies[0]["input"][1]["id"], "item_foreign_assistant_01");
    assert!(bodies[1]["input"][0].get("id").is_none());
    assert!(bodies[1]["input"][1].get("id").is_none());
    assert_eq!(bodies[1]["input"][2]["id"], "msg_native_01");
    assert_eq!(bodies[1]["input"][3]["id"], "item_function_01");
    assert_eq!(bodies[1]["input"][3]["call_id"], "call_function_01");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

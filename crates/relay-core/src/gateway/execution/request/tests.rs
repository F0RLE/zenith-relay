use super::prelude::requested_reasoning_effort;
use super::recovery::{adapter_error_response, should_wait_for_candidate_availability};
use super::translate::translate_basis_points_completed;
use crate::gateway::errors::AttemptFailure;
use crate::{AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi};
use axum::http::StatusCode;
use serde_json::json;

#[test]
fn basis_points_completed_json_and_stream_use_the_client_protocol() {
    let upstream = serde_json::to_vec(&json!({
        "id": "resp_1", "model": "test", "status": "completed",
        "output": [{"type":"message","id":"msg_1","role":"assistant","status":"completed",
            "content":[{"type":"output_text","text":"Hello","annotations":[]}]}],
        "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
    }))
    .unwrap();
    for client in WireApi::ALL {
        let input = match client {
            WireApi::Responses => json!({"model":"test","input":"Hi"}),
            WireApi::ChatCompletions => {
                json!({"model":"test","messages":[{"role":"user","content":"Hi"}]})
            }
            WireApi::Messages => {
                json!({"model":"test","max_tokens":64,"messages":[{"role":"user","content":"Hi"}]})
            }
            WireApi::Gemini => json!({"contents":[{"role":"user","parts":[{"text":"Hi"}]}]}),
        };
        for stream in [false, true] {
            let adapter = SourceAdapter::between(client, WireApi::Responses).unwrap();
            let prepared = adapter
                .prepare_request(AdapterRequestContext {
                    client_wire_api: client,
                    request: &input,
                    model: "test",
                    stream,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "test-account",
                    response_id_seed: "test-request",
                })
                .unwrap();
            let responses_request = prepared.upstream_body().clone();
            let result =
                translate_basis_points_completed(prepared, &upstream, &responses_request, stream)
                    .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&result.bytes).unwrap();
            let events = result.stream.map(|bytes| String::from_utf8(bytes).unwrap());
            match client {
                WireApi::Responses => {
                    assert_eq!(body["output"][0]["content"][0]["text"], "Hello");
                    if let Some(events) = &events {
                        assert!(events.contains("response.completed"));
                    }
                }
                WireApi::ChatCompletions => {
                    assert_eq!(body["choices"][0]["message"]["content"], "Hello");
                    if let Some(events) = &events {
                        assert!(events.contains("chat.completion.chunk"));
                    }
                }
                WireApi::Messages => {
                    assert_eq!(body["content"][0]["text"], "Hello");
                    if let Some(events) = &events {
                        assert!(events.contains("message_stop"));
                    }
                }
                WireApi::Gemini => {
                    assert_eq!(
                        body["candidates"][0]["content"]["parts"][0]["text"],
                        "Hello"
                    );
                    if let Some(events) = &events {
                        assert!(events.contains("candidates"));
                    }
                }
            }
        }
    }
}

#[test]
fn basis_points_messages_tool_call_is_unwrapped_before_protocol_translation() {
    let input = json!({
        "model": "test", "max_tokens": 64,
        "messages": [{"role":"user","content":"Find it"}],
        "tools": [{"name":"lookup","description":"Lookup","input_schema":{
            "type":"object","properties":{"q":{"type":"string"}}
        }}]
    });
    let code = json!({"tool":"lookup","args":{"q":"needle"}}).to_string();
    let upstream = serde_json::to_vec(&json!({
        "id": "resp_1", "model": "test", "status": "completed",
        "output": [{"type":"function_call","id":"fc_outer","call_id":"call_1",
            "name":"run_officejs","arguments":json!({"code":code}).to_string()}],
        "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
    }))
    .unwrap();
    for stream in [false, true] {
        let prepared = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &input,
                model: "test",
                stream,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "test-account",
                response_id_seed: "test-request",
            })
            .unwrap();
        let responses_request = prepared.upstream_body().clone();
        let result =
            translate_basis_points_completed(prepared, &upstream, &responses_request, stream)
                .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&result.bytes).unwrap();
        assert_eq!(body["content"][0]["type"], "tool_use");
        assert_eq!(body["content"][0]["name"], "lookup");
        assert_eq!(body["content"][0]["input"]["q"], "needle");
        if let Some(events) = result.stream {
            let events = String::from_utf8(events).unwrap();
            assert!(events.contains("tool_use"));
            assert!(events.contains("message_stop"));
        }
    }
}

#[test]
fn basis_points_messages_tool_result_keeps_the_call_link() {
    let input = json!({
        "model":"test", "max_tokens":64,
        "tools":[{"name":"lookup","input_schema":{"type":"object"}}],
        "messages":[
            {"role":"user","content":"Find it"},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"lookup","input":{"q":"needle"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"Found"}]}
        ]
    });
    let prepared = SourceAdapter::MessagesToResponses
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request: &input,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "test-account",
            response_id_seed: "test-request",
        })
        .unwrap();
    let basis_points =
        super::super::basis_points::prepare_request(prepared.upstream_body()).unwrap();
    let items = basis_points["input"].as_array().unwrap();
    assert!(items.iter().any(|item| {
        item["type"] == "function_call"
            && item["name"] == "run_officejs"
            && item["call_id"] == "call_1"
    }));
    assert!(items.iter().any(|item| {
        item["type"] == "function_call_output"
            && item["call_id"] == "call_1"
            && item["output"] == "Found"
    }));
}

#[tokio::test]
async fn adapter_error_response_exposes_safe_parameter_name() {
    let response = adapter_error_response(crate::AdapterError::parameter_unsupported_for(
        "text.verbosity",
    ));
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["param"], "text.verbosity");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("text.verbosity"));
    assert_eq!(body["error"]["code"], "adapter_parameter_unsupported");
}

#[test]
fn requested_reasoning_effort_uses_only_the_matching_client_contract() {
    let responses = json!({"reasoning": {"effort": " High "}});
    let chat = json!({"reasoning_effort": " Low "});

    assert_eq!(
        requested_reasoning_effort(&responses, WireApi::Responses),
        Some("high".to_string())
    );
    assert_eq!(
        requested_reasoning_effort(&chat, WireApi::ChatCompletions),
        Some("low".to_string())
    );
    assert_eq!(
        requested_reasoning_effort(&responses, WireApi::Messages),
        None
    );
    assert_eq!(
        requested_reasoning_effort(
            &json!({"reasoning": {"effort": "none"}}),
            WireApi::Responses
        ),
        None
    );
}

#[test]
fn retry_wait_is_opt_in_and_only_accepts_transient_failures() {
    let transient = Some(AttemptFailure::classified_with_hint(
        StatusCode::SERVICE_UNAVAILABLE,
        "upstream_unavailable",
        Default::default(),
    ));
    let auth = Some(AttemptFailure::classified_with_hint(
        StatusCode::UNAUTHORIZED,
        "upstream_unauthorized",
        Default::default(),
    ));
    let rejected = Some(AttemptFailure::classified_with_hint(
        StatusCode::BAD_REQUEST,
        "upstream_candidate_rejected",
        Default::default(),
    ));

    assert!(!should_wait_for_candidate_availability(
        false, &transient, false, false
    ));
    assert!(should_wait_for_candidate_availability(
        true, &transient, false, false
    ));
    assert!(!should_wait_for_candidate_availability(
        true, &auth, false, false
    ));
    assert!(!should_wait_for_candidate_availability(
        true, &rejected, false, false
    ));
}

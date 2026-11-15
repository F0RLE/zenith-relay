use super::wire_gateway_tests::{UPSTREAM_KEY, gateway_for, run_probe, serve, sse_reply};
use super::*;
use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use std::sync::{Arc, Mutex};

const TOOL: &str = "mcp__relay_contract__lookup";
const CALL: &str = "toolu_relay_fixture";
const RESULT: &str = "relay-tool-result-ok";
const ACK: &str = "relay-contract-ok";
type WireEvents = Vec<(&'static str, Value)>;

mod foreign;

#[derive(Clone)]
struct Fixture {
    protocol: WireApi,
    requests: Arc<Mutex<Vec<Value>>>,
}

fn messages_events(model: &Value, tool: bool) -> WireEvents {
    let mut events = vec![
        (
            "message_start",
            json!({"type":"message_start","message":{
                "id":"msg_fixture_cycle","type":"message","role":"assistant","model":model,
                "content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":0}
            }}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block": if tool {
            json!({"type":"tool_use","id":CALL,"name":TOOL,"input":{}})
        } else { json!({"type":"text","text":""}) }}),
        ),
    ];
    if tool {
        for fragment in ["{\"value\":", "\"synthetic\"}"] {
            events.push(("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":fragment}})));
        }
    } else {
        events.push(("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":ACK}})));
    }
    events.extend([
        ("content_block_stop", json!({"type":"content_block_stop","index":0})),
        ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":if tool {"tool_use"} else {"end_turn"},"stop_sequence":null},"usage":{"output_tokens":5}})),
        ("message_stop", json!({"type":"message_stop"})),
    ]);
    events
}

fn responses_events(model: &Value, tool: bool) -> WireEvents {
    let item = if tool {
        json!({"id":"fc_fixture_cycle","type":"function_call","call_id":CALL,"name":TOOL,"arguments":"{\"value\":\"synthetic\"}","status":"completed"})
    } else {
        json!({"id":"msg_fixture_cycle","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":ACK,"annotations":[]}]})
    };
    let mut started = item.clone();
    started["status"] = json!("in_progress");
    if tool {
        started["arguments"] = json!("");
    } else {
        started["content"] = json!([]);
    }
    let mut events = vec![
        (
            "response.created",
            json!({"type":"response.created","response":{"id":"resp_fixture_cycle","object":"response","status":"in_progress","model":model,"output":[]}}),
        ),
        (
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":started}),
        ),
    ];
    if tool {
        for fragment in ["{\"value\":", "\"synthetic\"}"] {
            events.push(("response.function_call_arguments.delta", json!({"type":"response.function_call_arguments.delta","item_id":"fc_fixture_cycle","output_index":0,"delta":fragment})));
        }
        events.push(("response.function_call_arguments.done", json!({"type":"response.function_call_arguments.done","item_id":"fc_fixture_cycle","output_index":0,"arguments":item["arguments"]})));
    } else {
        events.extend([
            ("response.content_part.added", json!({"type":"response.content_part.added","item_id":"msg_fixture_cycle","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})),
            ("response.output_text.delta", json!({"type":"response.output_text.delta","item_id":"msg_fixture_cycle","output_index":0,"content_index":0,"delta":ACK})),
            ("response.output_text.done", json!({"type":"response.output_text.done","item_id":"msg_fixture_cycle","output_index":0,"content_index":0,"text":ACK})),
            ("response.content_part.done", json!({"type":"response.content_part.done","item_id":"msg_fixture_cycle","output_index":0,"content_index":0,"part":item["content"][0]})),
        ]);
    }
    events.extend([
        ("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":item})),
        ("response.completed", json!({"type":"response.completed","response":{"id":"resp_fixture_cycle","object":"response","status":"completed","model":model,"output":[item],"usage":{"input_tokens":5,"output_tokens":5,"total_tokens":10}}})),
    ]);
    for (sequence, (_, value)) in events.iter_mut().enumerate() {
        value["sequence_number"] = json!(sequence);
    }
    events
}

fn assert_tool_result(body: &Value, protocol: WireApi) {
    if matches!(protocol, WireApi::ChatCompletions | WireApi::Gemini) {
        return foreign::assert_tool_result(body, protocol);
    }
    let result = if protocol == WireApi::Messages {
        let blocks: Vec<_> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|message| message["content"].as_array().into_iter().flatten())
            .collect();
        let call = blocks
            .iter()
            .find(|block| block["type"] == "tool_use" && block["id"] == CALL)
            .copied()
            .unwrap();
        let result = blocks
            .iter()
            .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == CALL)
            .copied()
            .unwrap();
        assert_eq!(call["name"], TOOL);
        assert_eq!(call["input"], json!({"value":"synthetic"}));
        assert_ne!(result["is_error"], true);
        &result["content"]
    } else {
        let items = body["input"].as_array().unwrap();
        let call = items
            .iter()
            .find(|item| item["type"] == "function_call" && item["call_id"] == CALL)
            .unwrap();
        let result = items
            .iter()
            .find(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
            .unwrap();
        assert_eq!(call["name"], TOOL);
        assert_eq!(
            serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
            json!({"value":"synthetic"})
        );
        &result["output"]
    };
    if let Some(text) = result.as_str() {
        assert_eq!(text, RESULT);
    } else {
        let content = result.as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], RESULT);
        assert!(matches!(
            content[0]["type"].as_str(),
            Some("text" | "input_text")
        ));
    }
}

async fn upstream(
    State(fixture): State<Fixture>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    Json(body): Json<Value>,
) -> axum::response::Response {
    if fixture.protocol == WireApi::Messages {
        assert_eq!(headers.get("x-api-key").unwrap(), UPSTREAM_KEY);
    } else if fixture.protocol == WireApi::Gemini {
        assert_eq!(headers.get("x-goog-api-key").unwrap(), UPSTREAM_KEY);
        assert_eq!(uri.path(), "/v1/models/gateway-model:streamGenerateContent");
        assert_eq!(uri.query(), Some("alt=sse"));
    } else {
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            format!("Bearer {UPSTREAM_KEY}")
        );
    }
    if fixture.protocol != WireApi::Gemini {
        assert_eq!(body["model"], "gateway-model");
        assert_eq!(body["stream"], true);
    }
    let tool = {
        let mut requests = fixture.requests.lock().unwrap();
        let tool = requests.len().is_multiple_of(2);
        requests.push(body.clone());
        tool
    };
    if !tool {
        assert_tool_result(&body, fixture.protocol);
    }
    sse_reply(
        match fixture.protocol {
            WireApi::Messages => messages_events(&body["model"], tool),
            WireApi::Responses => responses_events(&body["model"], tool),
            WireApi::ChatCompletions => foreign::chat_events(&body["model"], tool),
            WireApi::Gemini => foreign::gemini_events(tool),
            WireApi::GeminiInteractions => unreachable!("native-only protocol"),
        },
        fixture.protocol == WireApi::ChatCompletions,
    )
}

#[test]
#[ignore = "requires ZENITH_CLAUDE_CODE_BIN; isolated Code, deterministic MCP tool and synthetic upstreams"]
fn installed_claude_code_completes_gateway_tool_cycle() {
    let cli = std::env::var_os("ZENITH_CLAUDE_CODE_BIN")
        .expect("Set ZENITH_CLAUDE_CODE_BIN to the exact installed Claude Code executable");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    for protocol in [
        WireApi::Messages,
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let cases = if protocol == WireApi::Gemini {
            vec![("claude-opus-4-7", vec!["low", "high"], "high")]
        } else {
            vec![
                ("claude-opus-4-6", vec!["low", "high", "max"], "max"),
                (
                    "claude-opus-4-7",
                    vec!["low", "high", "xhigh", "max"],
                    "max,xhigh",
                ),
            ]
        };
        for (identity, levels, efforts) in cases {
            let mut configured = catalog_model_for(identity, &levels);
            configured.protocol_routes[0].upstream_wire_api = protocol;
            let models = inference_models_for_client(&[configured], true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let path = match protocol {
                WireApi::Messages => "/v1/messages",
                WireApi::Responses => "/v1/responses",
                WireApi::ChatCompletions => "/v1/chat/completions",
                WireApi::Gemini => "/v1/models/gateway-model:streamGenerateContent",
                WireApi::GeminiInteractions => unreachable!("native-only protocol"),
            };
            let upstream =
                runtime.block_on(serve(Router::new().route(path, post(upstream)).with_state(
                    Fixture {
                        protocol,
                        requests: requests.clone(),
                    },
                )));
            let (gateway, events) = gateway_for(&runtime, &upstream, identity, &levels, protocol);
            let mut command = installed_wire_command(&cli, &models[0], efforts);
            command.args(["--gateway", &gateway.origin, "--tool-cycle"]);
            run_probe(&mut command, &format!("{protocol:?}/{identity}/tool-cycle"));
            let expected: Vec<_> = efforts
                .split(',')
                .flat_map(|effort| [effort, effort])
                .collect();
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), expected.len());
            let pointer = match protocol {
                WireApi::Messages => "/output_config/effort",
                WireApi::Responses => "/reasoning/effort",
                WireApi::ChatCompletions => "/reasoning_effort",
                WireApi::Gemini => "/generationConfig/thinkingConfig/thinkingLevel",
                WireApi::GeminiInteractions => unreachable!("native-only protocol"),
            };
            for (index, (request, effort)) in requests.iter().zip(&expected).enumerate() {
                if protocol != WireApi::Gemini {
                    assert_eq!(request["model"], "gateway-model");
                }
                assert_eq!(request.pointer(pointer).unwrap(), *effort);
                if index % 2 == 1 {
                    assert_tool_result(request, protocol);
                }
            }
            let events = events.lock().unwrap();
            assert_eq!(events.len(), expected.len());
            for (event, effort) in events.iter().zip(expected) {
                assert!(event.success);
                assert_eq!(event.http_status, 200);
                assert_eq!(event.requested_model.as_deref(), models[0]["name"].as_str());
                assert_eq!(event.resolved_model.as_deref(), Some("gateway-model"));
                assert_eq!(event.requested_reasoning_effort.as_deref(), Some(effort));
                assert_eq!(event.effective_reasoning_effort.as_deref(), Some(effort));
                assert_eq!(event.input_tokens, Some(5));
                assert_eq!(event.output_tokens, Some(5));
            }
        }
    }
}

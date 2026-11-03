use super::*;

fn function(name: &str) -> Value {
    json!({"type":"function","name":name,"parameters":{
        "type":"object","properties":{},"additionalProperties":false}})
}

fn response(content: Value) -> Value {
    json!({"id":"resp_synthetic_integrity","model":"gpt-test","status":"completed","output":[{
        "type":"message","id":"msg_synthetic_integrity","role":"assistant","status":"completed",
        "content":[content]}]})
}

struct Outcome {
    status: StatusCode,
    terminal: Value,
    sent: Vec<Value>,
    body: String,
}

async fn execute(request: Value, reply: Value, upstream_stream: bool) -> Outcome {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    *crate::poison::mutex(&server.state.generation_body) = reply;
    server
        .state
        .stream_generations
        .store(upstream_stream, Ordering::SeqCst);
    server
        .state
        .strict_request_schema
        .store(true, Ordering::SeqCst);
    let runtime = Arc::new(runtime(&server).await);
    let (base, task) = super::recovery::gateway(runtime).await;
    let response = super::retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    task.abort();
    let terminal = if request["stream"] == true && status.is_success() {
        let terminal = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|event| {
                matches!(
                    event["type"].as_str(),
                    Some(
                        "response.completed" | "response.failed" | "response.incomplete" | "error"
                    )
                )
            })
            .expect("stream must have a terminal outcome");
        terminal.get("response").cloned().unwrap_or(terminal)
    } else {
        serde_json::from_str(&body).unwrap()
    };
    let sent = crate::poison::mutex(&server.state.generation_bodies).clone();
    assert_eq!(
        sent.len(),
        server.state.generation_requests.load(Ordering::SeqCst)
    );
    Outcome {
        status,
        terminal,
        sent,
        body,
    }
}

#[tokio::test]
async fn required_choices_preserve_refusals_in_json_and_sse_without_retry() {
    for stream in [false, true] {
        for upstream_stream in [false, true] {
            for choice in [
                json!("auto"),
                json!("required"),
                json!({"type":"function","name":"allowed"}),
                json!({"type":"allowed_tools","mode":"required","tools":[{"type":"function","name":"allowed"}]}),
            ] {
                let result = execute(
                    json!({"model":"gpt-test","input":"Synthetic request",
                    "tools":[function("allowed")],"tool_choice":choice,"stream":stream}),
                    response(json!({"type":"refusal","refusal":"Synthetic refusal"})),
                    upstream_stream,
                )
                .await;
                assert_eq!(
                    result.sent.len(),
                    1,
                    "completed refusal must not cause another generation"
                );
                assert_eq!(result.status, StatusCode::OK, "{}", result.terminal);
                assert_eq!(
                    result.terminal["status"], "completed",
                    "{}",
                    result.terminal
                );
                assert_eq!(
                    result.terminal["output"][0]["content"][0]["refusal"],
                    "Synthetic refusal"
                );
            }
        }
    }
}

#[tokio::test]
async fn explicit_catalog_removes_historical_permissions_but_preserves_replay() {
    for stream in [false, true] {
        for declaration in ["additional_tools", "tool_search_output"] {
            for tools in [json!([]), json!([function("allowed")])] {
                let request = json!({"model":"gpt-test","tools":tools,"stream":stream,"input":[
                    {"type":declaration,"tools":[function("retired")]},
                    {"type":"function_call","call_id":"call_synthetic_old","name":"retired","arguments":"{}"},
                    {"type":"function_call_output","call_id":"call_synthetic_old","output":"Synthetic old result"},
                    {"role":"user","content":"Synthetic next turn"}]});
                let call = json!({"id":"resp_synthetic_integrity","model":"gpt-test","status":"completed","output":[{
                    "type":"function_call","id":"fc_synthetic_new","status":"completed",
                    "call_id":"call_synthetic_new","name":"run_officejs",
                    "arguments":json!({"references":["retired"],"code":"{}"}).to_string()}]});
                let denied = execute(request.clone(), call, stream).await;
                assert_eq!(denied.sent.len(), 1);
                assert_ne!(
                    denied.terminal["status"], "completed",
                    "{}",
                    denied.terminal
                );
                assert!(
                    denied.terminal.get("error").is_some() || denied.terminal["type"] == "error"
                );
                let replay = execute(request,response(json!({"type":"output_text","text":"Synthetic next answer","annotations":[]})),stream).await;
                assert_eq!(replay.status, StatusCode::OK);
                assert_eq!(replay.terminal["status"], "completed");
                assert_eq!(replay.sent.len(), 1);
                let input = replay.sent[0]["input"].as_array().unwrap();
                assert!(input.iter().any(|item| item["type"] == "function_call"
                    && item["call_id"] == "call_synthetic_old"
                    && item["name"] == "run_officejs"));
                assert!(
                    input
                        .iter()
                        .any(|item| item["type"] == "function_call_output"
                            && item["call_id"] == "call_synthetic_old"
                            && item["output"] == "Synthetic old result")
                );
            }
        }
    }
}

#[tokio::test]
async fn unresolved_item_references_fail_before_generation() {
    for stream in [false, true] {
        for reference in [
            json!({"type":"item_reference","id":"msg_synthetic_unavailable"}),
            json!({"id":"msg_synthetic_unavailable"}),
        ] {
            let result = execute(
                json!({"model":"gpt-test","stream":stream,"input":[reference,
                {"role":"user","content":"Continue from the referenced synthetic item"}]}),
                response(json!({"type":"output_text","text":"Synthetic answer","annotations":[]})),
                stream,
            )
            .await;
            assert!(
                result.sent.is_empty(),
                "unresolved context must not spend a generation"
            );
            assert!(!result.status.is_success());
            assert_ne!(result.terminal["status"], "completed");
        }
    }
}

#[tokio::test]
async fn changed_tool_kinds_preserve_full_history_but_validate_new_calls() {
    let raw = "raw synthetic input\n\"quoted\" C:\\synthetic\\file";
    let native = |batched: bool, swapped: bool, id: &str| {
        let references = json!(["synthetic.execute", "synthetic.patch"]);
        let payloads = if swapped {
            json!([raw, {}])
        } else {
            json!([{}, raw])
        };
        let envelopes = if batched {
            vec![json!({"references":references,"code":format!("relay-tool-batch-v1\n{payloads}")})]
        } else {
            payloads.as_array().unwrap().iter().enumerate().map(|(index, payload)| {
                json!({"references":[references[index]],"code":payload.as_str().map(str::to_owned).unwrap_or_else(||payload.to_string())})
            }).collect()
        };
        json!({"id":format!("resp_{id}"),"model":"gpt-test","status":"completed",
            "output":envelopes.into_iter().enumerate().map(|(index, arguments)| {
                json!({"type":"function_call","name":"run_officejs","status":"completed",
                    "id":format!("fc_{id}_{index}"),"call_id":format!("call_{id}_{index}"),
                    "arguments":arguments.to_string()})
            }).collect::<Vec<_>>()})
    };
    for stream in [false, true] {
        for upstream_stream in [false, true] {
            for batched in [false, true] {
                let mut request = json!({"model":"gpt-test","stream":stream,
                    "parallel_tool_calls":true,"relay_bps_tool_catalog":"compact",
                    "input":[{"role":"user","content":"Synthetic task"}],
                    "tools":[{"type":"namespace","name":"synthetic","tools":[
                        function("execute"),{"type":"custom","name":"patch"}]}]});
                let original = native(batched, false, "synthetic_first");
                let first = execute(request.clone(), original.clone(), upstream_stream).await;
                assert_eq!(first.status, StatusCode::OK);
                assert_eq!(first.terminal["status"], "completed", "{}", first.terminal);
                assert_eq!(first.sent.len(), 1);
                let calls = first.terminal["output"].as_array().unwrap();
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0]["type"], "function_call");
                assert_eq!(calls[1]["type"], "custom_tool_call");
                assert_eq!(calls[1]["input"], raw);
                let history = request["input"].as_array_mut().unwrap();
                history.extend(calls.clone());
                for (index, call) in calls.iter().enumerate() {
                    history.push(
                        json!({"type":format!("{}_output",call["type"].as_str().unwrap()),
                        "call_id":call["call_id"],"output":format!("Synthetic result {index}")}),
                    );
                }
                history.push(json!({"role":"user","content":"Synthetic next turn"}));
                request["tools"][0]["tools"] = json!([
                    {"type":"custom","name":"execute"},function("patch")]);
                let next = execute(
                    request.clone(),
                    response(json!({"type":"output_text",
                    "text":"Synthetic next answer","annotations":[]})),
                    upstream_stream,
                )
                .await;
                assert_eq!(next.status, StatusCode::OK);
                assert_eq!(next.terminal["status"], "completed", "{}", next.terminal);
                assert_eq!(
                    next.terminal["output"][0]["content"][0]["text"],
                    "Synthetic next answer"
                );
                assert_eq!(next.sent.len(), 1);
                let input = next.sent[0]["input"].as_array().unwrap();
                let replayed = input
                    .iter()
                    .filter(|item| item["type"] == "function_call")
                    .collect::<Vec<_>>();
                let expected = original["output"].as_array().unwrap();
                assert_eq!(replayed.len(), expected.len());
                for (replay, original) in replayed.iter().zip(expected) {
                    let actual: Value =
                        serde_json::from_str(replay["arguments"].as_str().unwrap()).unwrap();
                    let expected: Value =
                        serde_json::from_str(original["arguments"].as_str().unwrap()).unwrap();
                    assert_eq!(actual["references"], expected["references"]);
                    assert_eq!(actual["code"], expected["code"]);
                    assert_eq!(replay["call_id"], original["call_id"]);
                    assert_eq!(
                        input
                            .iter()
                            .filter(|item| item["type"] == "function_call_output"
                                && item["call_id"] == replay["call_id"])
                            .count(),
                        1
                    );
                }
                // Historical encodings cannot bypass the replacement tool definitions.
                let denied = execute(
                    request.clone(),
                    native(batched, false, "synthetic_next"),
                    upstream_stream,
                )
                .await;
                assert_eq!(denied.sent.len(), 1);
                assert_ne!(
                    denied.terminal["status"], "completed",
                    "{}",
                    denied.terminal
                );
                assert!(
                    denied.terminal.get("error").is_some() || denied.terminal["type"] == "error"
                );
                let accepted = execute(
                    request,
                    native(batched, true, "synthetic_next"),
                    upstream_stream,
                )
                .await;
                assert_eq!(accepted.status, StatusCode::OK);
                assert_eq!(
                    accepted.terminal["status"], "completed",
                    "{}",
                    accepted.terminal
                );
                assert_eq!(accepted.sent.len(), 1);
                let calls = accepted.terminal["output"].as_array().unwrap();
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0]["type"], "custom_tool_call");
                assert_eq!(calls[0]["input"], raw);
                assert_eq!(calls[1]["type"], "function_call");
                assert_eq!(
                    serde_json::from_str::<Value>(calls[1]["arguments"].as_str().unwrap()).unwrap(),
                    json!({})
                );
                for (call, name) in calls.iter().zip(["execute", "patch"]) {
                    assert_eq!(call["name"], name);
                    assert_eq!(call["namespace"], "synthetic");
                }
            }
        }
    }
}

#[tokio::test]
async fn replayed_call_ids_fail_json_sse_without_releasing_tools_or_regenerating() {
    for stream in [false, true] {
        for upstream_stream in [false, true] {
            for mode in ["compact", "full", "discovery"] {
                let original = format!("call_{}", "synthetic".repeat(16));
                let native = |id: &str| {
                    json!({"id":"resp_synthetic","status":"completed","output":[{
                    "type":"function_call","name":"run_officejs","status":"completed",
                    "id":"fc_synthetic","call_id":id,
                    "arguments":json!({"references":["allowed"],"code":"{}"}).to_string()}]})
                };
                let mut request = json!({"model":"gpt-test","stream":stream,"relay_bps_tool_catalog":mode,
                    "tools":[function("allowed")],"input":[{"role":"user","content":"Synthetic task"}]});
                let first = execute(request.clone(), native(&original), upstream_stream).await;
                assert_eq!(first.status, StatusCode::OK);
                assert_eq!(first.terminal["status"], "completed", "{}", first.terminal);
                assert_eq!(first.sent.len(), 1);
                let call = first.terminal["output"][0].clone();
                assert_eq!(call["call_id"], original);
                request["input"].as_array_mut().unwrap().extend([
                    call,
                    json!({"type":"function_call_output","call_id":original,"output":"Synthetic result"}),
                    json!({"role":"user","content":"Synthetic next turn"}),
                ]);
                let next = execute(
                    request.clone(),
                    response(json!({"type":"output_text",
                    "text":"Synthetic next answer","annotations":[]})),
                    upstream_stream,
                )
                .await;
                assert_eq!(next.status, StatusCode::OK);
                assert_eq!(next.terminal["status"], "completed", "{}", next.terminal);
                assert_eq!(next.sent.len(), 1);
                let input = next.sent[0]["input"].as_array().unwrap();
                let call = input
                    .iter()
                    .find(|item| item["type"] == "function_call")
                    .unwrap();
                let replay_id = call["call_id"].as_str().unwrap();
                assert_ne!(replay_id, original);
                assert!(
                    input
                        .iter()
                        .any(|item| item["type"] == "function_call_output"
                            && item["call_id"] == replay_id
                            && item["output"] == "Synthetic result")
                );
                let mut invalid = native("call_synthetic_fresh");
                invalid["output"]
                    .as_array_mut()
                    .unwrap()
                    .push(native(replay_id)["output"][0].clone());
                let denied = execute(request, invalid, upstream_stream).await;
                assert_eq!(
                    denied.sent.len(),
                    1,
                    "invalid identity must not trigger another generation"
                );
                assert_ne!(
                    denied.terminal["status"], "completed",
                    "{}",
                    denied.terminal
                );
                assert!(
                    denied.terminal.get("error").is_some() || denied.terminal["type"] == "error"
                );
                for event in denied
                    .body
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                {
                    assert!(
                        !(event["type"] == "response.output_item.done"
                            && matches!(
                                event["item"]["type"].as_str(),
                                Some("function_call" | "custom_tool_call")
                            )),
                        "invalid identity must not release even the fresh call: {event}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn incoming_history_errors_fail_before_any_provider_generation() {
    for stream in [false, true] {
        for mode in ["compact", "full", "discovery"] {
            let original = format!("call_{}", "synthetic".repeat(16));
            let request = json!({"model":"gpt-test","stream":stream,"relay_bps_tool_catalog":mode,
                "tools":[function("allowed")],"input":[
                    {"type":"function_call","name":"allowed","call_id":original,"arguments":"{}"},
                    {"type":"function_call_output","call_id":original,"output":"Synthetic result"},
                    {"role":"user","content":"Synthetic next turn"}]});
            let reply = response(
                json!({"type":"output_text","text":"Synthetic next answer","annotations":[]}),
            );
            let valid = execute(request.clone(), reply.clone(), stream).await;
            assert_eq!(valid.status, StatusCode::OK);
            assert_eq!(valid.terminal["status"], "completed", "{}", valid.terminal);
            assert_eq!(valid.sent.len(), 1);
            let replay_id = valid.sent[0]["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["type"] == "function_call")
                .unwrap()["call_id"]
                .clone();
            assert_ne!(replay_id, original);
            let mut cases = Vec::new();
            for index in [0, 1] {
                let mut invalid = request.clone();
                invalid["input"]
                    .as_array_mut()
                    .unwrap()
                    .push(request["input"][index].clone());
                cases.push(invalid);
            }
            let mut mismatched = request.clone();
            mismatched["input"][1]["type"] = json!("custom_tool_call_output");
            cases.push(mismatched);
            let mut collision = request;
            collision["input"].as_array_mut().unwrap().extend([
                json!({"type":"function_call","name":"allowed","call_id":replay_id,"arguments":"{}"}),
                json!({"type":"function_call_output","call_id":replay_id,"output":"Synthetic other result"}),
            ]);
            cases.push(collision);
            for invalid in cases {
                let denied = execute(invalid, reply.clone(), stream).await;
                assert!(
                    denied.sent.is_empty(),
                    "ambiguous history must not spend a generation"
                );
                assert!(!denied.status.is_success());
                assert_ne!(denied.terminal["status"], "completed");
            }
        }
    }
}

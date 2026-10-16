use super::*;

fn count_request() -> Value {
    json!({
        "input": "Count synthetic entries",
        "tools": [{
            "type": "function",
            "name": "count_entries",
            "parameters": {
                "type": "object",
                "properties": {"count": {"type": "number", "minimum": 1}},
                "required": ["count"],
                "additionalProperties": false
            }
        }]
    })
}

#[derive(serde::Deserialize)]
struct CountArguments {
    count: usize,
}

#[test]
fn converted_function_arguments_accept_exact_integral_decimal_values() {
    let request = count_request();
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    assert_eq!(
        prepared.upstream_body()["tools"][0]["function"]["parameters"],
        request["tools"][0]["parameters"]
    );
    let translated = prepared
        .translate_response_bytes(
            &serde_json::to_vec(&json!({
                "id": "chat_count",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_count",
                            "type": "function",
                            "function": {"name": "count_entries", "arguments": "{\"count\":1000.0}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }))
            .unwrap(),
        )
        .unwrap()
        .unwrap();
    let call = &translated.response_body()["output"][0];
    assert_eq!(call["call_id"], "call_count");
    let arguments = call["arguments"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<CountArguments>(arguments)
            .unwrap()
            .count,
        1000
    );
    assert_eq!(arguments, "{\"count\":1000}");
    let continuation = translated.continuation().unwrap().1;
    assert!(matches!(
        &continuation.portable_history.as_ref().unwrap().last().unwrap().blocks[0],
        Block::ToolCall { arguments, .. } if arguments == "{\"count\":1000}"
    ));
}

#[test]
fn fragmented_integral_arguments_match_stream_deltas_and_completed_call() {
    let mut bridge = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &count_request(),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    for (index, fragment) in ["{\"count\":1", "000.", "0}"].into_iter().enumerate() {
        let function = if index == 0 {
            json!({"name": "count_entries", "arguments": fragment})
        } else {
            json!({"arguments": fragment})
        };
        bridge.push(
            format!(
                "data: {}\n\n",
                json!({
                    "id": "chat_count",
                    "choices": [{"index":0, "delta":{"tool_calls":[{
                        "index": 0, "id": "call_count", "type": "function", "function": function
                    }]}}]
                })
            )
            .as_bytes(),
        );
    }
    bridge.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n");
    bridge.finish();
    let completed = &bridge.completed().unwrap().response_body;
    let arguments = completed["output"][0]["arguments"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        serde_json::from_str::<CountArguments>(&arguments)
            .unwrap()
            .count,
        1000
    );
    let mut deltas = String::new();
    while let Some(frame) = bridge.pop_output() {
        let text = String::from_utf8(frame).unwrap();
        for line in text.lines().filter_map(|line| line.strip_prefix("data: ")) {
            let event: Value = serde_json::from_str(line).unwrap();
            if event["type"] == "response.function_call_arguments.delta" {
                deltas.push_str(event["delta"].as_str().unwrap());
            }
            if event["type"] == "response.function_call_arguments.done" {
                assert_eq!(event["arguments"], arguments);
            }
        }
    }
    assert_eq!(deltas, arguments);
}

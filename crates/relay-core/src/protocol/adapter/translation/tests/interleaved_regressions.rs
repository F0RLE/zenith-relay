//! Independently authored conversation regressions inspired by CLIProxyAPI's
//! interleaved-tool scenarios (MIT, inspected at 8289ec4). No upstream code.
use super::*;

#[test]
fn interleaved_calls_keep_identity_arguments_and_order_for_every_foreign_client() {
    for client in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
        for completed_order in [[0, 1], [1, 0]] {
            let arguments = [
                r#"{"value":1,"note":"λ\\quoted"}"#,
                r#"{"value":2,"note":"雪"}"#,
            ];
            let calls = (0..2)
                .map(|index| {
                    json!({"type":"function_call","id":format!("fc_interleaved_{index}"),
                    "call_id":format!("call_interleaved_{index}"),"name":"lookup",
                    "arguments":arguments[index],"status":"completed"})
                })
                .collect::<Vec<_>>();
            let mut events = vec![
                json!({"type":"response.created","response":{"id":"resp_interleaved","output":[]}}),
            ];
            for (index, call) in calls.iter().enumerate() {
                let mut initial = call.clone();
                initial["arguments"] = "".into();
                initial["status"] = "in_progress".into();
                events.push(json!({"type":"response.output_item.added","output_index":index,"item":initial}));
            }
            for index in [0, 1] {
                events.push(json!({"type":"response.function_call_arguments.delta","output_index":index,"delta":"{\"value\":"}));
            }
            for index in [1, 0] {
                events.push(
                    json!({"type":"response.function_call_arguments.delta","output_index":index,
                    "delta":arguments[index].strip_prefix("{\"value\":").unwrap()}),
                );
            }
            for index in completed_order {
                events.push(json!({"type":"response.output_item.done","output_index":index,"item":calls[index]}));
            }
            events.push(json!({"type":"response.completed","response":{"id":"resp_interleaved","status":"completed",
                "output":calls,"usage":{"input_tokens":13,"output_tokens":7,"total_tokens":20}}}));
            let bytes = events
                .iter()
                .map(|event| format!("data: {event}\r\n\r\n"))
                .collect::<String>()
                .into_bytes();
            let mut request = input(client);
            request["tools"] = tool_history(client)["tools"].clone();
            for width in [1, 7, 31] {
                let mut bridge = prepare(client, WireApi::Responses, &request, true)
                    .into_stream_bridge()
                    .unwrap();
                for chunk in bytes.chunks(width) {
                    bridge.push(chunk);
                }
                bridge.finish();
                let result = bridge
                    .completed()
                    .unwrap_or_else(|| panic!("{client:?}, {completed_order:?}, width={width}"));
                let decoded = response::decode(client, &result.response_body, "test").unwrap();
                assert_eq!(
                    (decoded.usage.input, decoded.usage.output),
                    (Some(13), Some(7))
                );
                let body = &result.response_body;
                let observed = match client {
                    WireApi::Messages => body["content"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|item| item["type"] == "tool_use")
                        .map(|item| {
                            (
                                item["id"].clone(),
                                item["name"].clone(),
                                item["input"].clone(),
                            )
                        })
                        .collect::<Vec<_>>(),
                    WireApi::ChatCompletions => body["choices"][0]["message"]["tool_calls"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|item| {
                            (
                                item["id"].clone(),
                                item["function"]["name"].clone(),
                                serde_json::from_str::<Value>(
                                    item["function"]["arguments"].as_str().unwrap(),
                                )
                                .unwrap(),
                            )
                        })
                        .collect(),
                    WireApi::Gemini => body["candidates"][0]["content"]["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|item| item.get("functionCall"))
                        .map(|call| {
                            (
                                call["id"].clone(),
                                call["name"].clone(),
                                call["args"].clone(),
                            )
                        })
                        .collect(),
                    WireApi::Responses | WireApi::GeminiInteractions => unreachable!(),
                };
                assert_eq!(observed.len(), 2, "{client:?}");
                for (index, (id, name, args)) in observed.iter().enumerate() {
                    assert_eq!(
                        id,
                        &json!(format!("call_interleaved_{index}")),
                        "{client:?}"
                    );
                    assert_eq!(name, "lookup");
                    assert_eq!(
                        args,
                        &serde_json::from_str::<Value>(arguments[index]).unwrap()
                    );
                }
            }
        }
    }
}

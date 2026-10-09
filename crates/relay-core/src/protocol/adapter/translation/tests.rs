use super::*;
use crate::{AdapterRequestContext, CacheWriteTtl, SourceAdapter};
use serde_json::json;
mod history;
mod matrix;
mod responses_usage;
mod terminal;
mod tools;

mod reasoning;

fn input(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => json!({"model":"test","input":"Hello"}),
        WireApi::ChatCompletions => {
            json!({"model":"test","messages":[{"role":"user","content":"Hello"}]})
        }
        WireApi::Messages => {
            json!({"model":"test","max_tokens":64,"messages":[{"role":"user","content":"Hello"}]})
        }
        WireApi::Gemini => json!({"contents":[{"role":"user","parts":[{"text":"Hello"}]}]}),
    }
}

fn output(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => {
            json!({"id":"response_test","object":"response","model":"test","status":"completed","output":[{"type":"message","id":"msg_test","role":"assistant","content":[{"type":"output_text","text":"Hi","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":2}})
        }
        WireApi::ChatCompletions => {
            json!({"id":"chat_test","object":"chat.completion","model":"test","choices":[{"index":0,"message":{"role":"assistant","content":"Hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}})
        }
        WireApi::Messages => {
            json!({"id":"msg_test","type":"message","role":"assistant","model":"test","content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}})
        }
        WireApi::Gemini => {
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}})
        }
    }
}

fn prepare(
    client: WireApi,
    upstream: WireApi,
    request: &Value,
    stream: bool,
) -> crate::PreparedAdapterRequest {
    let adapter = SourceAdapter::between(client, upstream).unwrap();
    adapter
        .prepare_request(AdapterRequestContext {
            client_wire_api: client,
            request,
            model: "test",
            stream,
            reasoning_mode: if adapter.is_passthrough() {
                MessagesReasoningMode::Disabled
            } else {
                MessagesReasoningMode::Adaptive
            },
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "source-test",
            response_id_seed: "request-test",
        })
        .unwrap_or_else(|error| panic!("{client:?} -> {upstream:?}: {error}"))
}

fn sse(protocol: WireApi) -> Vec<u8> {
    let events = match protocol {
        WireApi::Responses => vec![
            json!({"type":"response.created","response":{"id":"response_test","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_test","role":"assistant","content":[]}}),
            json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hi"}),
            json!({"type":"response.completed","response":output(protocol)}),
        ],
        WireApi::ChatCompletions => vec![
            json!({"id":"chat_test","choices":[{"index":0,"delta":{"role":"assistant","content":"Hi"},"finish_reason":null}]}),
            json!({"id":"chat_test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
            json!({"id":"chat_test","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
        ],
        WireApi::Messages => vec![
            json!({"type":"message_start","message":{"id":"msg_test","type":"message","model":"test","role":"assistant","content":[],"usage":{"input_tokens":3}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ],
        WireApi::Gemini => vec![output(protocol)],
    };
    let mut stream = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    if protocol == WireApi::ChatCompletions {
        stream.push_str("data: [DONE]\n\n");
    }
    stream.into_bytes()
}

fn tool_stream(protocol: WireApi) -> Vec<u8> {
    let mut events = Vec::new();
    match protocol {
        WireApi::Responses => {
            events.push(json!({"type":"response.created","response":{"id":"resp_tools","output":[]}}));
            let mut output = Vec::new();
            for index in 0..2 {
                let item = json!({"type":"function_call","id":format!("fc_{index}"),"call_id":format!("call_new_{index}"),"name":"lookup","arguments":""});
                events.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
                let mut complete = item;
                complete["arguments"] = format!("{{\"value\":{index}}}").into();
                output.push(complete);
            }
            for fragment in ["{\"value\":", "0}"] {
                for index in 0..2 {
                    events.push(json!({"type":"response.function_call_arguments.delta","output_index":index,"delta":if fragment == "0}" { format!("{index}}}") } else { fragment.into() }}));
                }
            }
            events.push(json!({"type":"response.completed","response":{"id":"resp_tools","status":"completed","output":output,"usage":{"input_tokens":11,"output_tokens":7}}}));
        }
        WireApi::ChatCompletions => {
            for index in 0..2 {
                events.push(json!({"id":"chat_tools","choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"id":format!("call_new_{index}"),"type":"function","function":{"name":"lookup","arguments":"{\"value\":"}}]}}]}));
            }
            for index in 0..2 {
                events.push(json!({"id":"chat_tools","choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"function":{"arguments":format!("{index}}}")}}]}}]}));
            }
            events.push(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":7}}));
        }
        WireApi::Messages => {
            events.push(json!({"type":"message_start","message":{"id":"msg_tools","usage":{"input_tokens":11}}}));
            for index in 0..2 {
                events.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":format!("call_new_{index}"),"name":"lookup","input":{}}}));
                for fragment in ["{\"value\":".to_string(), format!("{index}}}")] {
                    events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":fragment}}));
                }
                events.push(json!({"type":"content_block_stop","index":index}));
            }
            events.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}));
            events.push(json!({"type":"message_stop"}));
        }
        WireApi::Gemini => events.push(json!({"candidates":[{"index":0,"content":{"role":"model","parts":[
            {"functionCall":{"id":"call_new_0","name":"lookup","args":{"value":0}}},
            {"functionCall":{"id":"call_new_1","name":"lookup","args":{"value":1}}}
        ]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":11,"candidatesTokenCount":7}})),
    }
    let mut bytes = events
        .iter()
        .map(|value| format!("data: {value}\n\n"))
        .collect::<String>();
    if protocol == WireApi::ChatCompletions {
        bytes.push_str("data: [DONE]\n\n");
    }
    bytes.into_bytes()
}

fn tool_history(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => json!({"model":"test","instructions":"Be brief","input":[
            {"role":"user","content":"Check the test value"},
            {"type":"function_call","call_id":"call_first","name":"lookup","arguments":"{\"value\":1}"},
            {"type":"function_call_output","call_id":"call_first","output":"found"},
            {"type":"function_call","call_id":"call_second","name":"lookup","arguments":"{\"value\":2}"},
            {"type":"function_call_output","call_id":"call_second","output":"missing"}
        ],"tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"value":{"type":"integer"}}}}],"tool_choice":"auto"}),
        WireApi::ChatCompletions => json!({"model":"test","messages":[
            {"role":"system","content":"Be brief"},{"role":"user","content":"Check the test value"},
            {"role":"assistant","tool_calls":[{"id":"call_first","type":"function","function":{"name":"lookup","arguments":"{\"value\":1}"}}]},
            {"role":"tool","tool_call_id":"call_first","content":"found"},
            {"role":"assistant","tool_calls":[{"id":"call_second","type":"function","function":{"name":"lookup","arguments":"{\"value\":2}"}}]},
            {"role":"tool","tool_call_id":"call_second","content":"missing"}
        ],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"value":{"type":"integer"}}}}}],"tool_choice":"auto"}),
        WireApi::Messages => json!({"model":"test","max_tokens":64,"system":"Be brief","messages":[
            {"role":"user","content":"Check the test value"},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_first","name":"lookup","input":{"value":1}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_first","content":"found"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_second","name":"lookup","input":{"value":2}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_second","content":"missing"}]}
        ],"tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"value":{"type":"integer"}}}}],"tool_choice":{"type":"auto"}}),
        WireApi::Gemini => json!({"systemInstruction":{"parts":[{"text":"Be brief"}]},"contents":[
            {"role":"user","parts":[{"text":"Check the test value"}]},
            {"role":"model","parts":[{"functionCall":{"id":"call_first","name":"lookup","args":{"value":1}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"call_first","name":"lookup","response":{"result":"found"}}}]},
            {"role":"model","parts":[{"functionCall":{"id":"call_second","name":"lookup","args":{"value":2}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"call_second","name":"lookup","response":{"result":"missing"}}}]}
        ],"tools":[{"functionDeclarations":[{"name":"lookup","parametersJsonSchema":{"type":"object","properties":{"value":{"type":"integer"}}}}]}],"toolConfig":{"functionCallingConfig":{"mode":"AUTO"}}}),
    }
}

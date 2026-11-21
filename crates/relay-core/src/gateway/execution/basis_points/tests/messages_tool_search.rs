use super::*;
use crate::{AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi};

fn request() -> Value {
    json!({
        "model":"synthetic", "max_tokens":64,
        "tools":[
            {"name":"ToolSearch","input_schema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}},
            {"name":"write_file","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},
            {"name":"lookup","defer_loading":true,"input_schema":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}}
        ],
        "messages":[{"role":"user","content":"Update a synthetic file and look up a record"}]
    })
}

fn converted(request: &Value, stream: bool) -> crate::PreparedAdapterRequest {
    SourceAdapter::MessagesToResponses
        .prepare_request_with_capabilities(
            AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request,
                model: "synthetic",
                stream,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-tool-search",
                response_id_seed: "synthetic-tool-search",
            },
            &crate::model_metadata::ModelCapabilities {
                reasoning: Some(false),
                ..Default::default()
            },
        )
        .unwrap()
}

fn tool_reply(id: &str, name: &str, arguments: &Value) -> Value {
    json!({"id":"resp_synthetic","status":"completed","model":"synthetic",
        "output":[{"type":"function_call","name":"run_officejs","id":format!("fc_{id}"),
            "call_id":id,"arguments":json!({"references":[name],"code":arguments.to_string()}).to_string()}],
        "usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}})
}

#[test]
fn messages_deferred_tools_survive_basis_points_call_result_cycle() {
    for stream in [false, true] {
        let mut request = request();
        for (id, name, arguments, result) in [
            (
                "write_1",
                "write_file",
                json!({"path":"synthetic.txt"}),
                json!("updated"),
            ),
            (
                "search_1",
                "ToolSearch",
                json!({"query":"lookup"}),
                json!([{"type":"tool_reference","tool_name":"lookup"}]),
            ),
            (
                "lookup_1",
                "lookup",
                json!({"id":7}),
                json!("synthetic record"),
            ),
        ] {
            let adapter = converted(&request, stream);
            let body = adapter.upstream_body().clone();
            let (prepared, wire) = PreparedClientRequest::prepare(body, &mut None, false).unwrap();
            assert!(prompt_text(&wire).contains("run_officejs"));
            if name != "lookup" {
                assert!(
                    prepared
                        .translate_value(tool_reply("unloaded", "lookup", &json!({"id":7})))
                        .is_err()
                );
            }
            let response = prepared
                .translate_value(tool_reply(id, name, &arguments))
                .unwrap();
            let response = adapter
                .translate_response_bytes(&serde_json::to_vec(&response).unwrap())
                .unwrap()
                .unwrap();
            let response = response.response_body();
            assert_eq!(response["stop_reason"], "tool_use");
            assert_eq!(response["content"][0]["id"], id);
            assert_eq!(response["content"][0]["name"], name);
            assert_eq!(response["content"][0]["input"], arguments);
            request["messages"].as_array_mut().unwrap().extend([
                json!({"role":"assistant","content":response["content"]}),
                json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":result}]}),
            ]);
        }
        let adapter = converted(&request, stream);
        let (prepared, _) =
            PreparedClientRequest::prepare(adapter.upstream_body().clone(), &mut None, false)
                .unwrap();
        let response = prepared.translate_value(json!({"id":"resp_done","status":"completed","model":"synthetic",
            "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}],
            "usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}})).unwrap();
        let response = adapter
            .translate_response_bytes(&serde_json::to_vec(&response).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(response.response_body()["stop_reason"], "end_turn");
        assert_eq!(response.response_body()["content"][0]["text"], "Done");
    }
}

#[test]
fn messages_deferred_tools_keep_forced_selection_and_explicit_revocation() {
    for stream in [false, true] {
        let mut request = request();
        request["tool_choice"] = json!({"type":"tool","name":"lookup"});
        let adapter = converted(&request, stream);
        let body = adapter.upstream_body().clone();
        let (prepared, _) = PreparedClientRequest::prepare(body.clone(), &mut None, false).unwrap();
        let reply = tool_reply("lookup_1", "lookup", &json!({"id":7}));
        assert!(prepared.translate_value(reply.clone()).is_ok());
        assert!(
            prepared
                .translate_value(tool_reply(
                    "write_1",
                    "write_file",
                    &json!({"path":"synthetic.txt"})
                ))
                .is_err()
        );
        let mut revoked = body;
        revoked["tools"] = json!([]);
        revoked["tool_choice"] = json!("auto");
        let (prepared, wire) = PreparedClientRequest::prepare(revoked, &mut None, false).unwrap();
        assert!(!prompt_text(&wire).contains("Native run_officejs is intercepted"));
        assert!(prepared.translate_value(reply).is_err());
    }
}

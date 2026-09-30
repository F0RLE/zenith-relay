use super::*;

#[test]
fn converts_tools_images_and_tool_result_continuation() {
    let request = json!({"instructions":"Use tools.","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Inspect"},{"type":"input_image","image_url":"data:image/png;base64,YQ=="}]}],"tools":[{"type":"function","name":"run","parameters":{"type":"object"}}],"tool_choice":{"type":"function","name":"run"}});
    let prepared = prepare_responses_to_gemini_with_reasoning(
        &request,
        "gemini-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "route",
        "req-1",
    )
    .unwrap();
    assert_eq!(
        prepared.upstream_body["contents"][0]["parts"][1]["inlineData"]["mimeType"],
        "image/png"
    );
    assert_eq!(
        prepared.upstream_body["tools"][0]["functionDeclarations"][0]["name"],
        "run"
    );
    let response = translate_gemini_response(prepared,&json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"run","args":{"command":"pwd"},"id":"call_1"}}]}}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3}})).unwrap();
    assert_eq!(response.response_body["output"][0]["type"], "function_call");
    let continued = prepare_responses_to_gemini_with_reasoning(&json!({"previous_response_id":response.response_id,"input":[{"type":"function_call_output","call_id":"call_1","output":"/tmp"}]}),"gemini-test",false,MessagesReasoningMode::Disabled,Some(response.continuation),"route","req-2").unwrap();
    assert_eq!(
        continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["name"],
        "run"
    );
}

#[test]
fn namespaces_are_flattened_with_a_stable_alias_and_restored_on_continuation() {
    let prepared = prepare_responses_to_gemini_with_reasoning(
        &json!({
            "input": "lookup",
            "tools": [{
                "type": "namespace",
                "name": "weather",
                "tools": [{
                    "type": "function",
                    "name": "lookup",
                    "parameters": {"type": "object"}
                }]
            }],
            "tool_choice": {
                "type": "function",
                "namespace": "weather",
                "name": "lookup"
            }
        }),
        "gemini-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "route",
        "namespace-1",
    )
    .unwrap();
    let alias = prepared.upstream_body["tools"][0]["functionDeclarations"][0]["name"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(alias.starts_with("relay_ns_"));
    assert_eq!(
        prepared.upstream_body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][0],
        alias
    );

    let response = translate_gemini_response(
        prepared,
        &json!({
            "candidates":[{"content":{"parts":[{"functionCall":{
                "name": alias.clone(),
                "args": {"city":"Paris"},
                "id":"call-weather"
            }}]}}]
        }),
    )
    .unwrap();
    assert_eq!(response.response_body["output"][0]["name"], "lookup");
    assert_eq!(response.response_body["output"][0]["namespace"], "weather");
    let continued = prepare_responses_to_gemini_with_reasoning(
        &json!({
            "previous_response_id": response.response_id,
            "input": [{
                "type":"function_call_output",
                "call_id":"call-weather",
                "output":"sunny"
            }]
        }),
        "gemini-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(response.continuation),
        "route",
        "namespace-2",
    )
    .unwrap();
    assert_eq!(
        continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["name"],
        alias
    );
}

#[test]
fn function_result_preserves_text_and_media_parts() {
    let prepared = prepare_responses_to_gemini_with_reasoning(
        &json!({
            "input": "inspect",
            "tools": [{"type":"function","name":"run"}]
        }),
        "gemini-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "route",
        "req-1",
    )
    .unwrap();
    let response = translate_gemini_response(
        prepared,
        &json!({
            "candidates":[{"content":{"parts":[{"functionCall":{"name":"run","args":{},"id":"call-1"}}]}}]
        }),
    )
    .unwrap();
    let continued = prepare_responses_to_gemini_with_reasoning(
        &json!({
            "previous_response_id": response.response_id,
            "input": [{"type":"function_call_output","call_id":"call-1","output":[
                {"type":"input_text","text":"done"},
                {"type":"input_image","image_url":"data:image/png;base64,YQ=="}
            ]}]
        }),
        "gemini-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(response.continuation),
        "route",
        "req-2",
    )
    .unwrap();
    assert_eq!(
        continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["response"]
            ["output"],
        "done"
    );
    assert_eq!(
        continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["response"]["parts"]
            [0]["inlineData"]["mimeType"],
        "image/png"
    );
}

#[test]
fn converts_thinking_and_usage() {
    let prepared = prepare_responses_to_gemini_with_reasoning(
        &json!({"input":"think","reasoning":{"effort":"high"}}),
        "gemini-test",
        false,
        MessagesReasoningMode::Budget,
        None,
        "route",
        "req-1",
    )
    .unwrap();
    assert_eq!(
        prepared.upstream_body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        16384
    );
    let response = translate_gemini_response(prepared,&json!({"candidates":[{"content":{"parts":[{"thought":true,"text":"private"},{"text":"done"}]}}],"usageMetadata":{"thoughtsTokenCount":4}})).unwrap();
    assert_eq!(response.response_body["output"][0]["type"], "reasoning");
    assert_eq!(
        response.response_body["usage"]["output_tokens_details"]["reasoning_tokens"],
        4
    );
}

#[test]
fn bridge_preserves_json_schema_constraints_for_gemini() {
    let prepared = prepare_responses_to_gemini_with_reasoning(
        &json!({
            "input": "structured",
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {
                    "type": "object",
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "additionalProperties": false,
                    "properties": {
                        "query": {
                            "type": "string",
                            "pattern": ".+",
                            "$ref": "#/defs/query"
                        }
                    },
                    "required": ["query"]
                }
            }],
            "text": {
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "title": "Answer",
                        "description": "Structured answer",
                        "$defs": {"unused": {"type": "string"}},
                        "additionalProperties": false,
                        "properties": {
                            "answer": {
                                "type": "string",
                                "minLength": 1,
                                "$ref": "#/defs/answer"
                            },
                            "scores": {
                                "type": "array",
                                "items": {
                                    "type": "number",
                                    "minimum": 0,
                                    "maximum": 1
                                }
                            }
                        },
                        "required": ["answer"]
                    }
                }
            }
        }),
        "gemini-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "route",
        "schema-1",
    )
    .unwrap();

    let schema = &prepared.upstream_body["generationConfig"]["responseJsonSchema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["title"], "Answer");
    assert_eq!(schema["description"], "Structured answer");
    assert_eq!(schema["properties"]["answer"]["type"], "string");
    assert_eq!(schema["$defs"], json!({"unused":{"type":"string"}}));
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["answer"]["$ref"], "#/defs/answer");
    assert_eq!(schema["properties"]["answer"]["minLength"], 1);
    assert_eq!(schema["properties"]["scores"]["items"]["type"], "number");
    assert_eq!(schema["properties"]["scores"]["items"]["minimum"], 0);

    let parameters =
        &prepared.upstream_body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"];
    assert_eq!(parameters["properties"]["query"]["type"], "string");
    assert_eq!(parameters["properties"]["query"]["pattern"], ".+");
    assert_eq!(parameters["properties"]["query"]["$ref"], "#/defs/query");
    assert_eq!(parameters["additionalProperties"], false);
}

#[test]
fn partial_function_arguments_are_translated_to_responses_arguments() {
    let request = prepare_responses_to_gemini_with_reasoning(
        &json!({
            "input": "weather",
            "tools": [{"type":"function","name":"weather","parameters":{"type":"object"}}]
        }),
        "gemini-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "route",
        "partial",
    )
    .unwrap();
    let response = translate_gemini_response(
        request,
        &json!({
            "candidates":[{"content":{"parts":[{"functionCall":{
                "name":"weather",
                "partialArgs":[
                    {"jsonPath":"$.location","stringValue":"Paris"},
                    {"jsonPath":"$.units","stringValue":"C"}
                ]
            }}]}}]
        }),
    )
    .unwrap();
    assert_eq!(
        response.response_body["output"][0]["arguments"],
        r#"{"location":"Paris","units":"C"}"#
    );
}

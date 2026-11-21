use super::*;

const MODEL: &str = "synthetic-native-model";

fn schema() -> Value {
    serde_json::from_str(r#"{
        "type":"object",
        "properties":{
            "z":{"type":"integer","minimum":900719925474099312345678901234567890},
            "a":{"oneOf":[{"type":"string","title":"first"},{"type":"null","title":"second"}],"description":"Synthetic annotated union"},
            "m":{"type":"number","default":0.12345678901234567890123456789}
        },
        "required":["z","a","m"],
        "additionalProperties":false
    }"#).unwrap()
}

fn extension() -> Value {
    serde_json::from_str(
        r#"{
        "z":123456789012345678901234567890,
        "a":0.12345678901234567890123456789,
        "nested":{"last":-0.0,"first":1e30},
        "opaque":"synthetic-provider-state"
    }"#,
    )
    .unwrap()
}

fn path(protocol: WireApi) -> &'static str {
    match protocol {
        WireApi::Responses => "/v1/responses",
        WireApi::ChatCompletions => "/v1/chat/completions",
        WireApi::Messages => "/v1/messages",
        WireApi::Gemini => "/v1beta/models/synthetic-native-model:generateContent",
        WireApi::GeminiInteractions => "/v1beta/interactions",
    }
}

fn request(protocol: WireApi, schema: &Value) -> Value {
    let mut request = match protocol {
        WireApi::Responses => json!({"model":MODEL,"input":"Synthetic request",
            "tools":[{"type":"function","name":"synthetic_lookup","parameters":schema}]}),
        WireApi::ChatCompletions => json!({"model":MODEL,
            "messages":[{"role":"user","content":"Synthetic request"}],
            "tools":[{"type":"function","function":{"name":"synthetic_lookup","parameters":schema}}]}),
        WireApi::Messages => json!({"model":MODEL,"max_tokens":32,
            "messages":[{"role":"user","content":"Synthetic request"}],
            "tools":[{"name":"synthetic_lookup","input_schema":schema}]}),
        WireApi::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"text":"Synthetic request"}]}],
            "tools":[{"functionDeclarations":[{"name":"synthetic_lookup","parametersJsonSchema":schema}]}]})
        }
        WireApi::GeminiInteractions => json!({"model":MODEL,"input":"Synthetic request",
            "tools":[{"type":"function","name":"synthetic_lookup","parameters":schema}]}),
    };
    request["synthetic_native_extension"] = extension();
    request
}

fn schema_pointer(protocol: WireApi) -> &'static str {
    match protocol {
        WireApi::Responses | WireApi::GeminiInteractions => "/tools/0/parameters",
        WireApi::ChatCompletions => "/tools/0/function/parameters",
        WireApi::Messages => "/tools/0/input_schema",
        WireApi::Gemini => "/tools/0/functionDeclarations/0/parametersJsonSchema",
    }
}

fn response(protocol: WireApi, schema: &Value) -> Value {
    let mut response = match protocol {
        WireApi::Responses => {
            json!({"id":"synthetic-response","model":MODEL,"status":"completed","output":[],
            "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}})
        }
        WireApi::ChatCompletions => {
            json!({"id":"synthetic-chat","model":MODEL,"object":"chat.completion",
            "choices":[{"index":0,"message":{"role":"assistant","content":"Synthetic reply"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}})
        }
        WireApi::Messages => {
            json!({"id":"synthetic-message","type":"message","role":"assistant","model":MODEL,
            "content":[{"type":"text","text":"Synthetic reply"}],"stop_reason":"end_turn","stop_sequence":null,
            "usage":{"input_tokens":3,"output_tokens":2}})
        }
        WireApi::Gemini => {
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Synthetic reply"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}})
        }
        WireApi::GeminiInteractions => {
            json!({"id":"synthetic-interaction","model":MODEL,"status":"completed",
            "outputs":[{"type":"text","text":"Synthetic reply"}],
            "usage":{"total_input_tokens":3,"total_output_tokens":2,"total_tokens":5}})
        }
    };
    response["synthetic_native_extension"] = extension();
    response["synthetic_schema_extension"] = schema.clone();
    response
}

#[tokio::test]
async fn native_json_wire_preserves_schema_order_exact_numbers_and_opaque_extensions() {
    for protocol in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
        WireApi::Gemini,
        WireApi::GeminiInteractions,
    ] {
        let schema = schema();
        let expected_response = response(protocol, &schema);
        let reply = expected_response.clone();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = observed.clone();
        let upstream = spawn(Router::new().fallback(post(
            move |uri: axum::http::Uri, headers: HeaderMap, body: Bytes| {
                captured
                    .lock()
                    .unwrap()
                    .push((uri.path().to_string(), headers, body));
                let reply = reply.clone();
                async move { Json(reply).into_response() }
            },
        )))
        .await;
        let prefix = match protocol {
            WireApi::Gemini => "/v1beta",
            WireApi::GeminiInteractions => "/v1beta/interactions",
            _ => "/v1",
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "synthetic-native-source".into(),
                name: "Synthetic native source".into(),
                base_url: format!("{}{prefix}", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                proxy_url: None,
                wire_api: protocol,
                models: vec![MODEL.into()],
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "synthetic-local-key".into(),
                secret: LOCAL_KEY.into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(move |event| sink.lock().unwrap().push(event)),
        )
        .unwrap();
        let gateway = spawn(gateway::router(Arc::new(runtime))).await;
        let request = request(protocol, &schema);
        let send = reqwest::Client::new().post(format!("{}{}", gateway.base_url, path(protocol)));
        let send = match protocol {
            WireApi::Messages => send.header("x-api-key", LOCAL_KEY),
            WireApi::Gemini | WireApi::GeminiInteractions => {
                send.header("x-goog-api-key", LOCAL_KEY)
            }
            _ => send.bearer_auth(LOCAL_KEY),
        };
        let reply = send.json(&request).send().await.unwrap();
        let status = reply.status();
        let bytes = reply.bytes().await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{protocol:?}: {}",
            String::from_utf8_lossy(&bytes)
        );
        let received: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            serde_json::to_string(&received).unwrap(),
            serde_json::to_string(&expected_response).unwrap(),
            "{protocol:?}"
        );
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 1, "{protocol:?}");
        assert_eq!(observed[0].0, path(protocol), "{protocol:?}");
        let sent: Value = serde_json::from_slice(&observed[0].2).unwrap();
        assert_eq!(
            serde_json::to_string(sent.pointer(schema_pointer(protocol)).unwrap()).unwrap(),
            serde_json::to_string(&schema).unwrap(),
            "{protocol:?}"
        );
        assert_eq!(
            serde_json::to_string(&sent["synthetic_native_extension"]).unwrap(),
            serde_json::to_string(&request["synthetic_native_extension"]).unwrap(),
            "{protocol:?}"
        );
        let headers = &observed[0].1;
        match protocol {
            WireApi::Responses | WireApi::ChatCompletions => {
                assert_eq!(headers[AUTHORIZATION], format!("Bearer {SOURCE_KEY}"))
            }
            WireApi::Messages => {
                assert_eq!(headers["x-api-key"], SOURCE_KEY);
                assert!(headers.get(AUTHORIZATION).is_none());
            }
            WireApi::Gemini | WireApi::GeminiInteractions => {
                assert_eq!(headers["x-goog-api-key"], SOURCE_KEY);
                assert!(headers.get(AUTHORIZATION).is_none());
            }
        }
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1, "{protocol:?}");
        assert!(events[0].success, "{protocol:?}");
        assert_eq!(events[0].attempt, 1, "{protocol:?}");
    }
}

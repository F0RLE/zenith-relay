use super::*;
use axum::http::Uri;

#[derive(Clone)]
struct MatrixUpstream {
    protocol: WireApi,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
}

fn generation_path(protocol: WireApi, streaming: bool) -> &'static str {
    match (protocol, streaming) {
        (WireApi::Responses, _) => "/v1/responses",
        (WireApi::ChatCompletions, _) => "/v1/chat/completions",
        (WireApi::Messages, _) => "/v1/messages",
        (WireApi::Gemini, false) => "/v1beta/models/matrix-model:generateContent",
        (WireApi::Gemini, true) => "/v1beta/models/matrix-model:streamGenerateContent",
    }
}

fn request_body(protocol: WireApi, streaming: bool) -> Value {
    let mut request = match protocol {
        WireApi::Responses => json!({"input":"Synthetic matrix request"}),
        WireApi::ChatCompletions | WireApi::Messages => {
            json!({"messages":[{"role":"user","content":"Synthetic matrix request"}],"max_tokens":32})
        }
        WireApi::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"text":"Synthetic matrix request"}]}]})
        }
    };
    if protocol != WireApi::Gemini {
        request["model"] = "matrix-model".into();
        request["stream"] = streaming.into();
    }
    request
}

fn response_body(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => json!({
            "id":"resp_matrix","object":"response","status":"completed","model":"matrix-model",
            "output":[{"type":"message","id":"msg_matrix","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Matrix reply","annotations":[]}]}],
            "usage":{"input_tokens":13,"output_tokens":7,"total_tokens":20}
        }),
        WireApi::ChatCompletions => json!({
            "id":"chat_matrix","object":"chat.completion","model":"matrix-model",
            "choices":[{"index":0,"message":{"role":"assistant","content":"Matrix reply"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":13,"completion_tokens":7,"total_tokens":20}
        }),
        WireApi::Messages => json!({
            "id":"msg_matrix","type":"message","role":"assistant","model":"matrix-model",
            "content":[{"type":"text","text":"Matrix reply"}],"stop_reason":"end_turn",
            "usage":{"input_tokens":13,"output_tokens":7}
        }),
        WireApi::Gemini => json!({
            "candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Matrix reply"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":13,"candidatesTokenCount":5,"thoughtsTokenCount":2,"totalTokenCount":20}
        }),
    }
}

fn stream_body(protocol: WireApi) -> String {
    let events = match protocol {
        WireApi::Responses => vec![
            json!({"type":"response.created","response":{"id":"resp_matrix","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_matrix","role":"assistant","content":[]}}),
            json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Matrix reply"}),
            json!({"type":"response.completed","response":response_body(protocol)}),
        ],
        WireApi::ChatCompletions => vec![
            json!({"id":"chat_matrix","choices":[{"index":0,"delta":{"role":"assistant","content":"Matrix reply"},"finish_reason":null}]}),
            json!({"id":"chat_matrix","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":13,"completion_tokens":7,"total_tokens":20}}),
        ],
        WireApi::Messages => vec![
            json!({"type":"message_start","message":{"id":"msg_matrix","role":"assistant","model":"matrix-model","content":[],"usage":{"input_tokens":13}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Matrix reply"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ],
        WireApi::Gemini => vec![response_body(protocol)],
    };
    let mut body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    if protocol == WireApi::ChatCompletions {
        body.push_str("data: [DONE]\n\n");
    }
    body
}

async fn upstream(
    State(state): State<MatrixUpstream>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    let expected_key = match state.protocol {
        WireApi::Messages => headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok()),
        WireApi::Gemini => headers
            .get("x-goog-api-key")
            .and_then(|value| value.to_str().ok()),
        _ => headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer ")),
    };
    assert_eq!(expected_key, Some(SOURCE_KEY));
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false)
        || uri.path().ends_with(":streamGenerateContent");
    assert_eq!(uri.path(), generation_path(state.protocol, streaming));
    if state.protocol == WireApi::Gemini && streaming {
        assert_eq!(uri.query(), Some("alt=sse"));
    }
    state
        .requests
        .lock()
        .unwrap()
        .push((uri.path().to_owned(), body));
    if streaming {
        let chunks = stream_body(state.protocol)
            .as_bytes()
            .chunks(11)
            .map(|bytes| Ok::<_, Infallible>(Bytes::copy_from_slice(bytes)))
            .collect::<Vec<_>>();
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(stream::iter(chunks)))
            .unwrap()
    } else {
        Json(response_body(state.protocol)).into_response()
    }
}

#[tokio::test]
async fn all_sixteen_routes_execute_json_and_sse_with_actual_upstream_usage() {
    for output in WireApi::ALL {
        let observed = MatrixUpstream {
            protocol: output,
            requests: Arc::default(),
        };
        let provider = spawn(
            Router::new()
                .fallback(post(upstream))
                .with_state(observed.clone()),
        )
        .await;
        for input in WireApi::ALL {
            let mut source = RuntimeSource::unrestricted(ProviderSource {
                id: "matrix-source".into(),
                name: "Synthetic provider".into(),
                base_url: format!(
                    "{}/{}",
                    provider.base_url,
                    if output == WireApi::Gemini {
                        "v1beta"
                    } else {
                        "v1"
                    }
                ),
                api_key: SOURCE_KEY.into(),
                wire_api: output,
                models: vec!["matrix-model".into()],
            });
            source.protocol_bindings = vec![SourceProtocolBinding {
                wire_api: input,
                adapter: SourceAdapter::between(input, output).unwrap(),
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["matrix-model".into()],
            }];
            let events = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
            let sink = events.clone();
            let runtime = GatewayRuntime::from_pool(
                vec![source],
                vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                    id: "matrix-key".into(),
                    secret: LOCAL_KEY.into(),
                })],
                GatewayRuntimeOptions::default(),
                Arc::new(move |event| sink.lock().unwrap().push(event)),
            )
            .unwrap();
            let relay = spawn(gateway::router(Arc::new(runtime))).await;
            for streaming in [false, true] {
                let response = reqwest::Client::new()
                    .post(format!(
                        "{}{}",
                        relay.base_url,
                        generation_path(input, streaming)
                    ))
                    .bearer_auth(LOCAL_KEY)
                    .json(&request_body(input, streaming))
                    .send()
                    .await
                    .unwrap();
                let status = response.status();
                let body = response.text().await.unwrap();
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{input:?} -> {output:?} stream={streaming}: {body}"
                );
                assert!(
                    body.contains("Matrix reply"),
                    "{input:?} -> {output:?}: {body}"
                );
                assert!(
                    !body.contains("adapter_upstream"),
                    "{input:?} -> {output:?}: {body}"
                );
                if streaming {
                    let terminal = match input {
                        WireApi::Responses => "response.completed",
                        WireApi::ChatCompletions => "[DONE]",
                        WireApi::Messages => "message_stop",
                        WireApi::Gemini => "STOP",
                    };
                    assert!(body.contains(terminal), "{input:?} -> {output:?}: {body}");
                }
            }
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2, "{input:?} -> {output:?}");
            for event in events.iter() {
                assert!(
                    event.success,
                    "{input:?} -> {output:?}: {:?}",
                    event.error_category
                );
                assert_eq!(event.wire_api, input);
                assert_eq!(event.input_tokens, Some(13));
                assert_eq!(event.output_tokens, Some(7));
                assert_eq!(event.cached_input_tokens, None);
            }
        }
        assert_eq!(observed.requests.lock().unwrap().len(), 8);
    }
}

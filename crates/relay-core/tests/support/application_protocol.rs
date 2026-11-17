use super::*;
use axum::extract::ws::{Message as ServerMessage, WebSocketUpgrade};
use zenith_relay_core::UpstreamProtocolPolicy;

type Observations = Arc<Mutex<Vec<WireApi>>>;

fn completed(id: usize) -> Value {
    json!({"id":format!("resp_synthetic_{id}"),"object":"response","status":"completed",
        "model":"gpt-test","output":[{"id":format!("msg_synthetic_{id}"),"type":"message",
        "role":"assistant","status":"completed","content":[{"type":"output_text",
        "text":"Synthetic answer","annotations":[]}]}],
        "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}})
}

async fn responses_http(
    State(observed): State<Observations>,
    Json(body): Json<Value>,
) -> Response<Body> {
    let id = {
        let mut observed = observed.lock().unwrap();
        observed.push(WireApi::Responses);
        observed.len()
    };
    let response = completed(id);
    if body["stream"] == true {
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "data: {}\n\n",
                json!({"type":"response.completed","response":response})
            )))
            .unwrap()
    } else {
        Json(response).into_response()
    }
}

async fn responses_socket(
    State(observed): State<Observations>,
    upgrade: WebSocketUpgrade,
) -> Response<Body> {
    upgrade.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            match message {
                ServerMessage::Text(_) | ServerMessage::Binary(_) => {
                    let id = {
                        let mut observed = observed.lock().unwrap();
                        observed.push(WireApi::Responses);
                        observed.len()
                    };
                    let response = completed(id);
                    for event in [json!({"type":"response.created","response":{
                        "id":response["id"],"object":"response","model":"gpt-test","status":"in_progress","output":[]}}),
                        json!({"type":"response.completed","response":response})] {
                        if socket.send(ServerMessage::Text(event.to_string().into())).await.is_err() { return; }
                    }
                }
                ServerMessage::Ping(payload) => { if socket.send(ServerMessage::Pong(payload)).await.is_err() { return; } }
                ServerMessage::Close(_) => return,
                ServerMessage::Pong(_) => {}
            }
        }
    }).into_response()
}

async fn chat_http(
    State(observed): State<Observations>,
    Json(body): Json<Value>,
) -> Response<Body> {
    observed.lock().unwrap().push(WireApi::ChatCompletions);
    if body["stream"] == true {
        let frames = [
            json!({"id":"chat_synthetic","object":"chat.completion.chunk","model":"gpt-test",
                "choices":[{"index":0,"delta":{"role":"assistant","content":"Synthetic answer"},"finish_reason":null}]}),
            json!({"id":"chat_synthetic","object":"chat.completion.chunk","model":"gpt-test",
                "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
        ];
        let mut stream = frames
            .iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect::<String>();
        stream.push_str("data: [DONE]\n\n");
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(stream))
            .unwrap()
    } else {
        Json(json!({"id":"chat_synthetic","object":"chat.completion","model":"gpt-test",
            "choices":[{"index":0,"message":{"role":"assistant","content":"Synthetic answer"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}})).into_response()
    }
}

async fn setup(
    native_socket: bool,
    with_chat: bool,
) -> (TestServer, TestServer, Arc<GatewayRuntime>, Observations) {
    let observed = Observations::default();
    let responses = if native_socket {
        post(responses_http).get(responses_socket)
    } else {
        post(responses_http)
    };
    let upstream = spawn(
        Router::new()
            .route("/v1/responses", responses)
            .route("/v1/chat/completions", post(chat_http))
            .with_state(observed.clone()),
    )
    .await;
    let sources = [WireApi::Responses, WireApi::ChatCompletions]
        .into_iter()
        .filter(|wire| *wire == WireApi::Responses || with_chat)
        .map(|wire_api| {
            let mut source = RuntimeSource::unrestricted(ProviderSource {
                id: format!("synthetic-{wire_api:?}"),
                name: "Synthetic source".into(),
                base_url: format!("{}/v1", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                proxy_url: None,
                wire_api,
                models: vec!["gpt-test".into()],
            });
            source
                .protocol_config
                .capabilities
                .push(ModelEndpointCapability {
                    model_id: "gpt-test".into(),
                    upstream_wire_api: wire_api,
                    status: CapabilityStatus::Declared,
                    origin: CapabilityOrigin::Catalog,
                    checked_at_ms: 1,
                    features: Default::default(),
                    reasoning_efforts: Vec::new(),
                    token_limits: Default::default(),
                    service_tiers: None,
                });
            source
        })
        .collect();
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            sources,
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "application-key".into(),
                secret: LOCAL_KEY.into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap(),
    );
    runtime.set_key_upstream_protocol("application-key", UpstreamProtocolPolicy::Responses);
    let server = spawn(gateway::router(runtime.clone())).await;
    (upstream, server, runtime, observed)
}

#[tokio::test]
async fn application_protocol_http_retained_model_survives_catalog_change_without_route_recovery() {
    for stream in [false, true] {
        let (_upstream, server, runtime, observed) = setup(false, false).await;
        let client = reqwest::Client::new();
        let first: Value = client
            .post(format!("{}/v1/responses", server.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":"gpt-test","input":"Synthetic start"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        runtime.set_key_upstream_protocol("application-key", UpstreamProtocolPolicy::Messages);
        let catalog: Value = client
            .get(format!("{}/v1/models", server.base_url))
            .bearer_auth(LOCAL_KEY)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(catalog["data"], json!([]));
        let continued = client.post(format!("{}/v1/responses",server.base_url)).bearer_auth(LOCAL_KEY)
            .json(&json!({"model":"gpt-test","previous_response_id":first["id"],"input":"Synthetic next turn","stream":stream}))
            .send().await.unwrap();
        let status = continued.status();
        let body = continued.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("Synthetic answer"), "{body}");
        let fresh = client
            .post(format!("{}/v1/responses", server.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":"gpt-test","input":"New independent conversation"}))
            .send()
            .await
            .unwrap();
        assert_eq!(fresh.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            *observed.lock().unwrap(),
            vec![WireApi::Responses, WireApi::Responses]
        );
    }
}

#[tokio::test]
async fn application_protocol_websocket_new_turn_uses_saved_format_without_reconnecting() {
    for native_socket in [false, true] {
        let (_upstream, server, runtime, observed) = setup(native_socket, true).await;
        let upgraded = reqwest::Client::new()
            .get(format!("{}/v1/responses", server.base_url))
            .bearer_auth(LOCAL_KEY)
            .upgrade()
            .send()
            .await
            .unwrap();
        let mut socket = upgraded.into_websocket().await.unwrap();
        for policy in [
            UpstreamProtocolPolicy::Responses,
            UpstreamProtocolPolicy::ChatCompletions,
        ] {
            runtime.set_key_upstream_protocol("application-key", policy);
            socket
                .send(ClientWsMessage::Text(
                    json!({"type":"response.create","model":"gpt-test",
                "input":"New independent synthetic conversation"})
                    .to_string(),
                ))
                .await
                .unwrap();
            loop {
                let message =
                    receive_websocket_data_with_timeout(&mut socket, Duration::from_secs(10)).await;
                let payload = match message {
                    ClientWsMessage::Text(text) => text,
                    ClientWsMessage::Binary(bytes) => String::from_utf8(bytes.to_vec()).unwrap(),
                    _ => unreachable!(),
                };
                let event: Value = serde_json::from_str(&payload).unwrap();
                assert_ne!(event["type"], "error", "{event}");
                if event["type"] == "response.completed" {
                    break;
                }
            }
        }
        assert_eq!(
            *observed.lock().unwrap(),
            vec![WireApi::Responses, WireApi::ChatCompletions]
        );
        SinkExt::close(&mut socket).await.unwrap();
    }
}

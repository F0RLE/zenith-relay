use super::*;
use axum::extract::ws::{Message as ServerMessage, WebSocketUpgrade};

#[tokio::test]
async fn native_responses_preserves_images_in_tool_results_and_user_messages() {
    for streaming in [false, true] {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = observed.clone();
        let upstream = spawn(Router::new().fallback(post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body.clone());
            async move {
                let response = json!({
                    "id":"resp_synthetic_image", "status":"completed", "output":[],
                    "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}
                });
                if body["stream"] == true {
                    Response::builder()
                        .header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(format!(
                            "event: response.completed\ndata: {}\n\n",
                            json!({"type":"response.completed","response":response})
                        )))
                        .unwrap()
                } else {
                    Json(response).into_response()
                }
            }
        })))
        .await;
        let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
        let image = json!({
            "type":"input_image", "detail":"auto",
            "image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAMAAAADCAIAAADZSiLoAAAAFUlEQVR4nGP4z8AAQWDq/38YCywGAKxxCvaOcyXdAAAAAElFTkSuQmCC"
        });
        let request = json!({
            "model":"gpt-test", "stream":streaming, "store":false,
            "tools":[{"type":"function","name":"image_fixture","parameters":{"type":"object","properties":{}}}],
            "input":[
                {"role":"user","content":"Inspect the synthetic tool image."},
                {"type":"function_call","call_id":"call_synthetic_image","name":"image_fixture","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_synthetic_image","output":[
                    {"type":"input_text","text":"Image read successfully"},image.clone()
                ]},
                {"role":"user","content":[{"type":"input_text","text":"Inspect the same image as user input."},image]}
            ]
        });
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("resp_synthetic_image"));
        let bodies = observed.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["input"], request["input"]);
        assert_eq!(bodies[0]["tools"], request["tools"]);
        assert_eq!(bodies[0]["stream"], streaming);
        assert!(events.lock().unwrap().iter().any(|event| event.success));
    }
}

#[tokio::test]
async fn native_gemini_continuation_resumes_only_with_its_original_model_and_contents() {
    for first_streaming in [false, true] {
        for resume_streaming in [false, true] {
            let observed = Arc::new(Mutex::new(Vec::new()));
            let captured = observed.clone();
            let upstream = spawn(Router::new().fallback(post(
                move |uri: axum::http::Uri, Json(body): Json<Value>| {
                    captured.lock().unwrap().push(body.clone());
                    async move {
                        let mut candidate = json!({"index":0,"content":{"role":"model","parts":[{"text":"Synthetic reply"}]},"finishReason":"STOP"});
                        if body.get("continuationToken").is_none() {
                            candidate["finishReason"] = "CONTINUATION".into();
                            candidate["continuationToken"] = "c3ludGhldGlj".into();
                        }
                        let response = json!({"candidates":[candidate],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}});
                        if uri.path().ends_with(":streamGenerateContent") {
                            Response::builder()
                                .header(CONTENT_TYPE, "text/event-stream")
                                .body(Body::from(format!("data: {response}\n\n")))
                                .unwrap()
                        } else {
                            Json(response).into_response()
                        }
                    }
                },
            )))
            .await;
            let (gateway, events) = spawn_native_gemini_gateway(&upstream.base_url).await;
            let client = reqwest::Client::new();
            let path = |model: &str, streaming| {
                format!(
                    "{}/v1beta/models/{model}:{}",
                    gateway.base_url,
                    if streaming {
                        "streamGenerateContent"
                    } else {
                        "generateContent"
                    }
                )
            };
            let original = json!({"contents":[{"role":"user","parts":[{"text":"Synthetic original prompt"}]}],"cachedContent":"cachedContents/synthetic"});
            let response = client
                .post(path("gemini-test", first_streaming))
                .header("x-goog-api-key", LOCAL_KEY)
                .json(&original)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.text().await.unwrap().contains("c3ludGhldGlj"));
            let mut resume = original.clone();
            resume["continuationToken"] = "c3ludGhldGlj".into();
            for (model, changed) in [
                ("other-model", resume.clone()),
                ("gemini-test", {
                    let mut changed = resume.clone();
                    changed["contents"][0]["parts"][0]["text"] = "Changed prompt".into();
                    changed
                }),
                ("gemini-test", {
                    let mut changed = resume.clone();
                    changed["continuationToken"] = "b3RoZXI=".into();
                    changed
                }),
            ] {
                let response = client
                    .post(path(model, resume_streaming))
                    .bearer_auth(LOCAL_KEY)
                    .json(&changed)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::CONFLICT);
                let body: Value = response.json().await.unwrap();
                assert_eq!(body["error"]["code"], "response_continuation_unavailable");
            }
            // A path-only first request and an explicit matching model share one owner.
            resume["model"] = "gemini-test".into();
            let response = client
                .post(path("gemini-test", resume_streaming))
                .bearer_auth(LOCAL_KEY)
                .json(&resume)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.text().await.unwrap().contains("STOP"));
            let requests = observed.lock().unwrap();
            assert_eq!(requests.len(), 2);
            for request in requests.iter() {
                assert_eq!(request["contents"], original["contents"]);
                assert_eq!(request["cachedContent"], original["cachedContent"]);
                assert!(request.get("model").is_none());
            }
            assert_eq!(
                requests[1]["continuationToken"],
                resume["continuationToken"]
            );
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(
                events[0].error_category.as_deref(),
                Some("response_incomplete")
            );
            assert!(events[1].success);
            assert!(events.iter().all(|event| event.input_tokens == Some(3)));
        }
    }
}

#[tokio::test]
async fn native_websocket_preserves_raw_text_binary_numbers_and_repeated_beta_headers() {
    const CREATED: &str = r#"{"type":"response.created","response":{"id":"synthetic-response","status":"in_progress"}}"#;
    const EXTENSION: &str = r#"{ "type":"vendor_extension", "z":1e3, "a":123456789012345678901234567890, "p":0.12345678901234567890123456789 }"#;
    const DELTA: &str = r#"{"type":"response.output_text.delta","output_index":0,"delta":"synthetic answer","vendor_signature":"opaque-fixture"}"#;
    const COMPLETED: &str = r#"{"type":"response.completed","response":{"id":"synthetic-response","status":"completed","output":[],"usage":{"input_tokens":3,"output_tokens":2}}}"#;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let upstream = spawn(Router::new().route(
        "/v1/responses",
        get(move |headers: HeaderMap, upgrade: WebSocketUpgrade| {
            captured.lock().unwrap().push(headers);
            async move {
                upgrade.on_upgrade(|mut socket| async move {
                    assert!(matches!(
                        socket.recv().await,
                        Some(Ok(ServerMessage::Text(_)))
                    ));
                    for frame in [
                        ServerMessage::Text(CREATED.into()),
                        ServerMessage::Text(EXTENSION.into()),
                        ServerMessage::Binary(Bytes::from_static(DELTA.as_bytes())),
                        ServerMessage::Text(COMPLETED.into()),
                    ] {
                        socket.send(frame).await.unwrap();
                    }
                    // Keep this native transport alive until the client closes it.
                    while socket.recv().await.is_some() {}
                })
            }
        }),
    ))
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("openai-beta", "synthetic-feature-a")
        .header("openai-beta", "synthetic-feature-b")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type":"response.create","model":"gpt-test","input":"synthetic"}).to_string(),
        ))
        .await
        .unwrap();
    for (expected, binary) in [
        (CREATED, false),
        (EXTENSION, false),
        (DELTA, true),
        (COMPLETED, false),
    ] {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match frame {
            ClientWsMessage::Text(text) => {
                assert!(!binary);
                assert_eq!(text.as_bytes(), expected.as_bytes());
            }
            ClientWsMessage::Binary(bytes) => {
                assert!(binary);
                assert_eq!(bytes.as_ref(), expected.as_bytes());
            }
            _ => panic!("expected an application frame"),
        }
    }
    let headers = seen.lock().unwrap();
    assert_eq!(headers.len(), 1);
    let values = headers[0].get_all("openai-beta").iter().collect::<Vec<_>>();
    assert_eq!(values[0], "synthetic-feature-a");
    assert_eq!(values[1], "synthetic-feature-b");
    assert_eq!(headers[0][AUTHORIZATION], format!("Bearer {SOURCE_KEY}"));
    assert!(events.lock().unwrap().iter().any(|event| event.success));
}

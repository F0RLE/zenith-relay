use super::*;
use axum::extract::ws::{Message as ServerMessage, WebSocketUpgrade};

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

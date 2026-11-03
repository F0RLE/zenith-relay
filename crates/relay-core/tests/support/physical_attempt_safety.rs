use super::*;
use futures_util::StreamExt;

async fn broken_body(status: StatusCode, payload: Value) -> TestServer {
    broken_wire_body(status, payload.to_string(), "application/json").await
}

async fn broken_wire_body(
    status: StatusCode,
    payload: String,
    content_type: &'static str,
) -> TestServer {
    spawn(Router::new().route(
        "/v1/responses",
        post(move || {
            let payload = payload.clone();
            async move {
                let chunks = stream::unfold(0u8, move |index| {
                    let payload = payload.clone();
                    async move {
                        match index {
                            0 => Some((Ok::<_, io::Error>(Bytes::from(payload)), 1)),
                            1 => {
                                // Deliver a complete document before interrupting HTTP framing.
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                Some((Err(io::Error::other("synthetic interruption")), 2))
                            }
                            _ => None,
                        }
                    }
                });
                Response::builder()
                    .status(status)
                    .header(CONTENT_TYPE, content_type)
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }
        }),
    ))
    .await
}

fn completed_response() -> Value {
    json!({
        "id":"resp_synthetic", "object":"response", "status":"completed",
        "model": MODEL,
        "output":[{"type":"message","id":"msg_synthetic","role":"assistant",
            "status":"completed","content":[{"type":"output_text","text":"synthetic","annotations":[]}]}],
        "usage":{"input_tokens":5,"output_tokens":7,"total_tokens":12}
    })
}

#[tokio::test]
async fn truncated_429_with_generation_never_falls_back() {
    let upstream = broken_body(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error":{"code":"insufficient_quota"},
            "usage":{"input_tokens":5,"output_tokens":7}}),
    )
    .await;
    let (reserve, reserve_state) = spawn_upstream(
        "second-key",
        vec![response_reply("must-not-repeat", "no-store")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("first", &upstream, "first-key", &[MODEL], 10),
            source("second", &reserve, "second-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;
    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    response.bytes().await.unwrap();
    assert!(reserve_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), 1);
    assert!(!attempts[0].success);
    assert_eq!(attempts[0].input_tokens, Some(5));
    assert_eq!(attempts[0].output_tokens, Some(7));
    assert_eq!(attempts[0].error_category.as_deref(), Some("upstream_body"));
}

#[tokio::test]
async fn truncated_429_sse_with_generated_usage_never_falls_back() {
    let upstream = broken_wire_body(
        StatusCode::TOO_MANY_REQUESTS,
        "data: {\"usage\":{\"input_tokens\":5,\"output_tokens\":7}}\n\n".into(),
        "text/event-stream",
    )
    .await;
    let (reserve, reserve_state) = spawn_upstream(
        "second-key",
        vec![response_reply("must-not-repeat", "no-store")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("first", &upstream, "first-key", &[MODEL], 10),
            source("second", &reserve, "second-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;
    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    response.bytes().await.unwrap();
    assert!(reserve_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), 1);
    assert!(!attempts[0].success);
    assert_eq!(attempts[0].input_tokens, Some(5));
    assert_eq!(attempts[0].output_tokens, Some(7));
}

#[tokio::test]
async fn truncated_429_with_unparsed_json_never_proves_safe_fallback() {
    assert_interrupted_rejection(
        r#"{"usage":{"output_tokens":7},"output":["#,
        "application/json",
        false,
    )
    .await;
}

#[tokio::test]
async fn truncated_429_with_unframed_sse_never_proves_safe_fallback() {
    assert_interrupted_rejection(
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"insufficient_quota\"},\"usage\":{\"output_tokens\":7}}}",
        "text/event-stream",
        false,
    )
    .await;
}

#[tokio::test]
async fn truncated_429_with_complete_sse_rejection_keeps_safe_fallback() {
    assert_interrupted_rejection(
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"insufficient_quota\"},\"usage\":{\"output_tokens\":0}}}\n\n",
        "text/event-stream",
        true,
    )
    .await;
}

async fn assert_interrupted_rejection(payload: &str, content_type: &'static str, safe: bool) {
    let upstream =
        broken_wire_body(StatusCode::TOO_MANY_REQUESTS, payload.into(), content_type).await;
    let (reserve, reserve_state) = spawn_upstream(
        "second-key",
        vec![response_reply("must-not-repeat", "no-store")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("first", &upstream, "first-key", &[MODEL], 10),
            source("second", &reserve, "second-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;
    let response = request(&gateway, false).await;
    assert_eq!(
        response.status(),
        if safe {
            StatusCode::OK
        } else {
            StatusCode::BAD_GATEWAY
        },
        "{content_type}"
    );
    response.bytes().await.unwrap();
    assert_eq!(
        reserve_state.requests.lock().unwrap().len(),
        usize::from(safe)
    );
    let events = events.lock().unwrap();
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), if safe { 2 } else { 1 });
    assert!(!attempts[0].success);
    assert_eq!(attempts[0].input_tokens, None);
    assert_eq!(attempts[0].output_tokens, safe.then_some(0));
}

#[tokio::test]
async fn truncated_429_without_generation_keeps_proven_rejection_fallback() {
    let upstream = broken_body(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error":{"code":"insufficient_quota"},
            "usage":{"input_tokens":5,"output_tokens":0}}),
    )
    .await;
    let (reserve, reserve_state) = spawn_upstream(
        "second-key",
        vec![response_reply("safe-fallback", "no-store")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("first", &upstream, "first-key", &[MODEL], 10),
            source("second", &reserve, "second-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;
    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    assert_eq!(reserve_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].output_tokens, Some(0));
    assert!(!attempts[0].success);
    assert!(attempts[1].success);
}

#[tokio::test]
async fn buffered_body_error_retains_received_usage_without_success_or_replay() {
    for usage_known in [true, false] {
        let mut payload = completed_response();
        if !usage_known {
            payload.as_object_mut().unwrap().remove("usage");
        }
        let upstream = broken_body(StatusCode::OK, payload).await;
        let (reserve, reserve_state) = spawn_upstream(
            "second-key",
            vec![response_reply("must-not-repeat", "no-store")],
        )
        .await;
        let (gateway, events) = spawn_gateway(
            vec![
                source("first", &upstream, "first-key", &[MODEL], 10),
                source("second", &reserve, "second-key", &[MODEL], 0),
            ],
            vec![local_key("key", LOCAL_KEY, None)],
            3,
        )
        .await;
        let response = request(&gateway, false).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        response.bytes().await.unwrap();
        assert!(reserve_state.requests.lock().unwrap().is_empty());
        let events = events.lock().unwrap();
        let attempts = usage_observations::physical_attempts(&events);
        assert_eq!(attempts.len(), 1);
        assert!(!attempts[0].success);
        assert_eq!(attempts[0].input_tokens, usage_known.then_some(5));
        assert_eq!(attempts[0].output_tokens, usage_known.then_some(7));
        assert_eq!(attempts[0].error_category.as_deref(), Some("upstream_body"));
    }
}

#[tokio::test]
async fn cancellation_before_output_records_one_physical_send_and_releases_lease() {
    for before_headers in [false, true] {
        let polled = Arc::new(tokio::sync::Notify::new());
        let signal = polled.clone();
        let upstream = spawn(Router::new().route(
            "/v1/responses",
            post(move || {
                let signal = signal.clone();
                async move {
                    if before_headers {
                        signal.notify_one();
                        std::future::pending::<()>().await;
                    }
                    let chunks = stream::once(async move {
                        signal.notify_one();
                        std::future::pending::<Result<Bytes, io::Error>>().await
                    });
                    Response::builder()
                        .header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from_stream(chunks))
                        .unwrap()
                }
            }),
        ))
        .await;
        let captured = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
        let sink = captured.clone();
        let runtime = Arc::new(
            GatewayRuntime::from_pool(
                vec![source("first", &upstream, "first-key", &[MODEL], 10)],
                vec![local_key("key", LOCAL_KEY, None)],
                GatewayRuntimeOptions::default(),
                Arc::new(move |event| sink.lock().unwrap().push(event)),
            )
            .unwrap(),
        );
        let gateway = spawn(gateway::router(runtime.clone())).await;
        let url = format!("{}/v1/responses", gateway.base_url);
        let pending = tokio::spawn(async move {
            reqwest::Client::new()
                .post(url)
                .bearer_auth(LOCAL_KEY)
                .json(&json!({"model":MODEL,"input":"synthetic","stream":true}))
                .send()
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), polled.notified())
            .await
            .unwrap();
        assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 1);
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(3), async {
            while runtime.candidate_runtime_order()[0].in_flight != 0
                || captured.lock().unwrap().is_empty()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancellation must release the lease and settle telemetry");
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        let attempts = usage_observations::physical_attempts(&events);
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].attempt, 1);
        assert!(!attempts[0].success);
        assert_eq!(
            attempts[0].error_category.as_deref(),
            Some("client_cancelled")
        );
        assert_eq!(attempts[0].input_tokens, None);
        assert_eq!(attempts[0].output_tokens, None);
        assert_eq!(attempts[0].ttft_ms, None);
    }
}

#[tokio::test]
async fn transport_control_receives_usage_before_body_error() {
    for status in [StatusCode::OK, StatusCode::TOO_MANY_REQUESTS] {
        let upstream = broken_body(status, completed_response()).await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", upstream.base_url))
            .json(&json!({"model":MODEL,"input":"synthetic"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let mut body = response.bytes_stream();
        let mut bytes = Vec::new();
        let mut interrupted = false;
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(chunk) => bytes.extend_from_slice(&chunk),
                Err(_) => {
                    interrupted = true;
                    break;
                }
            }
        }
        assert!(interrupted);
        let payload: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["usage"]["output_tokens"], 7);
    }
}

#[tokio::test]
async fn healthy_response_control_records_usage_once() {
    let (upstream, _) = spawn_upstream(
        "first-key",
        vec![Reply::Json {
            status: StatusCode::OK,
            body: completed_response(),
            cache_control: "no-store",
            retry_after: None,
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![source("first", &upstream, "first-key", &[MODEL], 10)],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;
    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].output_tokens, Some(7));
    assert!(attempts[0].success);
}

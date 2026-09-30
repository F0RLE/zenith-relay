use super::*;
use std::convert::Infallible;
use tokio::sync::mpsc;

const LONG_PAUSE: Duration = Duration::from_secs(20 * 60);

#[derive(Clone)]
struct DeferredHttp {
    entered: Arc<Notify>,
    headers: Arc<Notify>,
    body_polled: Arc<Notify>,
    chunks: Arc<Mutex<Option<mpsc::UnboundedReceiver<Bytes>>>>,
    streaming: bool,
}

async fn deferred_http(State(state): State<DeferredHttp>) -> Response<Body> {
    state.entered.notify_one();
    state.headers.notified().await;
    let receiver = state.chunks.lock().unwrap().take().unwrap();
    let polled = state.body_polled;
    let chunks = stream::unfold(receiver, move |mut receiver| {
        let polled = polled.clone();
        async move {
            polled.notify_one();
            receiver
                .recv()
                .await
                .map(|bytes| (Ok::<_, Infallible>(bytes), receiver))
        }
    });
    Response::builder()
        .header(
            CONTENT_TYPE,
            if state.streaming {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(Body::from_stream(chunks))
        .unwrap()
}

async fn deferred_http_upstream(
    streaming: bool,
) -> (TestServer, DeferredHttp, mpsc::UnboundedSender<Bytes>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let state = DeferredHttp {
        entered: Arc::default(),
        headers: Arc::default(),
        body_polled: Arc::default(),
        chunks: Arc::new(Mutex::new(Some(receiver))),
        streaming,
    };
    let server = spawn(
        Router::new()
            .route("/v1/responses", post(deferred_http))
            .with_state(state.clone()),
    )
    .await;
    (server, state, sender)
}

async fn gateway_for_long_request(
    upstream: &TestServer,
    use_account: bool,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let authority = ready_authority("slow-account", "synthetic-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        if use_account {
            Vec::new()
        } else {
            vec![source("slow-source", upstream, "synthetic-key", 0)]
        },
        if use_account {
            vec![account("slow-account", "synthetic-account", upstream, 0)]
        } else {
            Vec::new()
        },
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    (gateway, events)
}

async fn advance_past_old_timeouts() {
    tokio::time::pause();
    tokio::time::advance(LONG_PAUSE).await;
    // Let the HTTP/WS tasks observe all expired timers without waiting in real time.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    tokio::time::resume();
}

fn assert_held(gateway: &TestServer, events: &Mutex<Vec<UsageEvent>>) {
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        1
    );
    assert!(events.lock().unwrap().is_empty());
}

fn assert_completed(gateway: &TestServer, events: &Mutex<Vec<UsageEvent>>) {
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        0
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(2));
    assert_eq!(events[0].retry_at_ms, None);
}

#[tokio::test]
async fn http_generation_waits_for_delayed_headers_and_first_output() {
    for use_account in [false, true] {
        for streaming in [false, true] {
            for delay_headers in [false, true] {
                let (upstream, state, sender) = deferred_http_upstream(streaming).await;
                let (gateway, events) = gateway_for_long_request(&upstream, use_account).await;
                let url = format!("{}/v1/responses", gateway.base_url);
                let pending = tokio::spawn(async move {
                    reqwest::Client::new()
                        .post(url)
                        .bearer_auth(LOCAL_KEY)
                        .json(&json!({"model": MODEL, "input": "synthetic", "stream": streaming}))
                        .send()
                        .await
                        .unwrap()
                });
                tokio::time::timeout(Duration::from_secs(2), state.entered.notified())
                    .await
                    .unwrap();
                if !delay_headers {
                    state.headers.notify_one();
                    tokio::time::timeout(Duration::from_secs(2), state.body_polled.notified())
                        .await
                        .unwrap();
                }

                advance_past_old_timeouts().await;
                assert!(!pending.is_finished(), "a quiet generation was interrupted");
                assert_held(&gateway, &events);
                let response = json!({"id":"slow-response","object":"response","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}});
                let bytes = if streaming {
                    format!(
                        "data: {}\n\n",
                        json!({"type":"response.completed","response":response})
                    )
                } else {
                    response.to_string()
                };
                state.headers.notify_one();
                sender.send(Bytes::from(bytes)).unwrap();
                drop(sender);
                let response = tokio::time::timeout(Duration::from_secs(2), pending)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert!(response.text().await.unwrap().contains("slow-response"));
                assert_completed(&gateway, &events);
            }
        }
    }
}

#[tokio::test]
async fn http_client_cancellation_releases_a_request_waiting_for_first_output() {
    let (upstream, state, _sender) = deferred_http_upstream(true).await;
    let (gateway, events) = gateway_for_long_request(&upstream, false).await;
    let url = format!("{}/v1/responses", gateway.base_url);
    let pending = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL,"input":"synthetic","stream":true}))
            .send()
            .await
    });
    state.headers.notify_one();
    tokio::time::timeout(Duration::from_secs(2), state.body_polled.notified())
        .await
        .unwrap();
    assert_held(&gateway, &events);
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        while gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled HTTP request retained its lease before output");
}

#[tokio::test]
async fn websocket_http_fallback_waits_through_silence_and_keeps_one_attempt() {
    let (upstream, state, sender) = deferred_http_upstream(true).await;
    let (gateway, events) = gateway_for_long_request(&upstream, false).await;
    let mut socket = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type":"response.create","model":MODEL,"input":"synthetic"}).to_string(),
        ))
        .await
        .unwrap();
    state.headers.notify_one();
    tokio::time::timeout(Duration::from_secs(2), state.body_polled.notified())
        .await
        .unwrap();

    advance_past_old_timeouts().await;
    assert_held(&gateway, &events);
    sender
        .send(Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
        ))
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    advance_past_old_timeouts().await;
    assert_held(&gateway, &events);
    sender
        .send(Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"slow-response\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ))
        .unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "slow-response"
    );
    assert_completed(&gateway, &events);
}

#[tokio::test]
async fn websocket_generation_waits_before_and_after_first_output() {
    for after_output in [false, true] {
        let barrier = Arc::new(Barrier::new(2));
        let release = Arc::new(Notify::new());
        let behavior = if after_output {
            WebSocketBehavior::Hold(release.clone())
        } else {
            WebSocketBehavior::GatedSuccess(barrier.clone())
        };
        let (upstream, state) = spawn_websocket_upstream_with_behavior(behavior).await;
        let (gateway, events) = gateway_for_long_request(&upstream, true).await;
        let mut socket = reqwest::Client::new()
            .get(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .upgrade()
            .send()
            .await
            .unwrap()
            .into_websocket()
            .await
            .unwrap();
        socket
            .send(ClientWsMessage::Text(
                json!({"type":"response.create","model":MODEL,"input":"synthetic"}).to_string(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.requests.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if after_output {
            assert_eq!(
                receive_websocket_json(&mut socket).await["type"],
                "response.output_text.delta"
            );
        }

        advance_past_old_timeouts().await;
        assert_held(&gateway, &events);
        if after_output {
            release.notify_one();
        } else {
            barrier.wait().await;
        }
        let response = receive_websocket_completion(&mut socket).await;
        assert_eq!(response["type"], "response.completed");
        assert_eq!(state.requests.lock().unwrap().len(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert_eq!(events[0].retry_at_ms, None);
        assert_eq!(
            gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
            0
        );
    }
}

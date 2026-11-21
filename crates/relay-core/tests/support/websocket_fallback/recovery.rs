use super::*;
use axum::response::IntoResponse;

#[tokio::test]
async fn slow_handshake_handoff_keeps_reservation_and_advances_rotation_once() {
    let upgrade_keys = Arc::new(Mutex::new(Vec::<String>::new()));
    let post_keys = Arc::new(Mutex::new(Vec::<String>::new()));
    let upstream = spawn(
        Router::new().route(
            "/v1/responses",
            get({
                let upgrade_keys = upgrade_keys.clone();
                move |headers: HeaderMap, upgrade: WebSocketUpgrade| {
                    upgrade_keys
                        .lock()
                        .unwrap()
                        .push(headers[AUTHORIZATION].to_str().unwrap().to_string());
                    async move {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        upgrade.on_upgrade(|_socket| async {})
                    }
                }
            })
            .post({
                let post_keys = post_keys.clone();
                move |headers: HeaderMap| {
                    let mut keys = post_keys.lock().unwrap();
                    keys.push(headers[AUTHORIZATION].to_str().unwrap().to_string());
                    let response_id = format!("synthetic-reservation-{}", keys.len());
                    async move {
                        Response::builder().header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(format!("data: {}\n\n", json!({
                            "type":"response.completed", "response":{
                                "id":response_id,"status":"completed","output":[],
                                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                            }
                        })))).unwrap()
                    }
                }
            }),
        ),
    )
    .await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![
            source("source-a", &upstream, "synthetic-a", 10),
            source("source-b", &upstream, "synthetic-b", 10),
        ],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
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
    for index in 1..=2 {
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type":"response.create", "model":MODEL,"input":"synthetic request"
                })
                .to_string(),
            ))
            .await
            .unwrap();
        // The optional handshake itself can consume two seconds before HTTP starts.
        let completion =
            receive_websocket_completion_with_timeout(&mut socket, Duration::from_secs(8)).await;
        assert_eq!(
            completion["response"]["id"],
            format!("synthetic-reservation-{index}")
        );
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let upgrades = upgrade_keys.lock().unwrap();
    let posts = post_keys.lock().unwrap();
    assert_eq!(upgrades.len(), 1);
    assert_eq!(posts.len(), 2);
    assert_eq!(
        upgrades[0], posts[0],
        "fallback must retain its reserved candidate"
    );
    assert_ne!(
        posts[0], posts[1],
        "the next request advances rotation exactly once"
    );
    let events = events.lock().unwrap();
    assert_eq!(
        events.len(),
        2,
        "an unsent handshake is not a generation attempt"
    );
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
    assert_ne!(events[0].request_id, events[1].request_id);
    assert_ne!(events[0].candidate_id, events[1].candidate_id);
    assert!(
        gateway
            .runtime
            .as_ref()
            .unwrap()
            .candidate_runtime_order()
            .iter()
            .all(|candidate| candidate.in_flight == 0)
    );
}

#[tokio::test]
async fn http_handoff_keeps_repairs_from_handshake_and_live_rejections() {
    for live_rejection in [false, true] {
        for lane in [None, Some("synthetic-lane")] {
            check_repaired_handoff(live_rejection, lane).await;
        }
    }
}

async fn check_repaired_handoff(live_rejection: bool, lane: Option<&str>) {
    let upgrades = Arc::new(AtomicUsize::new(0));
    let http_bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let error = json!({"error":{"message":"Missing required field: call_id"}});
    let upstream = spawn(Router::new().route(
        "/v1/responses",
        get({
            let upgrades = upgrades.clone();
            let error = error.clone();
            move |upgrade: WebSocketUpgrade| {
                let first = upgrades.fetch_add(1, Ordering::SeqCst) == 0;
                let error = error.clone();
                async move {
                    if !first {
                        return (StatusCode::METHOD_NOT_ALLOWED, "use HTTP").into_response();
                    }
                    if !live_rejection {
                        return (StatusCode::BAD_REQUEST, Json(error)).into_response();
                    }
                    upgrade.on_upgrade(move |mut socket| async move {
                        let Some(Ok(AxumWsMessage::Text(payload))) = socket.recv().await else {
                            return;
                        };
                        let body: Value = serde_json::from_str(&payload).unwrap();
                        assert!(body["input"][0].get("call_id").is_none());
                        let _ = socket.send(AxumWsMessage::Text(json!({
                            "type":"error", "status":400, "error":error["error"]
                        }).to_string().into())).await;
                    })
                }
            }
        })
        .post({
            let http_bodies = http_bodies.clone();
            move |Json(body): Json<Value>| {
                http_bodies.lock().unwrap().push(body.clone());
                async move {
                    if body["input"][0].get("call_id").is_none() {
                        return (StatusCode::BAD_REQUEST, Json(json!({
                            "error":{"message":"Missing required field: call_id"}
                        }))).into_response();
                    }
                    Response::builder().header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(format!("data: {}\n\n", json!({
                            "type":"response.completed",
                            "response":{"id":"synthetic-repaired-handoff","status":"completed",
                                "output":[],"usage":{"input_tokens":4,"output_tokens":1,"total_tokens":5}}
                        })))).unwrap()
                }
            }
        }),
    )).await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("handoff-source", &upstream, "synthetic-key", 10)],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    let mut request = json!({
        "type":"response.create", "model":MODEL,
        "input":[
            {"type":"function_call","name":"lookup","arguments":"{}"},
            {"type":"function_call_output","output":"synthetic result"}
        ]
    });
    if let Some(lane) = lane {
        request["stream_id"] = json!(lane);
    }
    socket
        .send(ClientWsMessage::Text(request.to_string()))
        .await
        .unwrap();
    let completed = receive_websocket_completion(&mut socket).await;
    assert_eq!(completed["response"]["id"], "synthetic-repaired-handoff");
    assert_eq!(completed.get("stream_id").and_then(Value::as_str), lane);
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert_eq!(upgrades.load(Ordering::SeqCst), 2);
    let bodies = http_bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        1,
        "HTTP must receive the repaired body on its first dispatch"
    );
    let call_id = bodies[0]["input"][0]["call_id"].as_str().unwrap();
    assert!(!call_id.is_empty());
    assert_eq!(bodies[0]["input"][1]["call_id"], call_id);
    assert_eq!(bodies[0]["input"][1]["output"], "synthetic result");
    drop(bodies);

    let events = events.lock().unwrap();
    let expected_attempts = if live_rejection { 2 } else { 1 };
    assert_eq!(events.len(), expected_attempts);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.request_id, events[0].request_id);
        assert_eq!(event.candidate_id, events[0].candidate_id);
        assert_eq!(event.attempt, (index + 1) as u16);
    }
    if live_rejection {
        assert_eq!(
            events[0].routing.as_ref().unwrap().attempt_outcome,
            Some(zenith_relay_core::AttemptOutcome::RetryRepair)
        );
    }
    let completed = events.last().unwrap();
    assert!(completed.success);
    assert_eq!(completed.input_tokens, Some(4));
    assert_eq!(
        completed.routing.as_ref().unwrap().attempt_outcome,
        Some(zenith_relay_core::AttemptOutcome::Completed)
    );
    let runtime = gateway.runtime.as_ref().unwrap();
    assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 0);
}

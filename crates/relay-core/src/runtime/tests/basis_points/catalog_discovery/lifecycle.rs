use super::*;

#[tokio::test]
async fn discovery_releases_completed_reader_before_waiting_for_next_headers() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    server
        .state
        .stream_generations
        .store(true, Ordering::SeqCst);
    let (closed, closure) = tokio::sync::oneshot::channel();
    crate::poison::mutex(&server.state.stream_closures).push_back(closed);
    let second_headers = Arc::new(Semaphore::new(0));
    crate::poison::mutex(&server.state.generation_header_gates)
        .extend([Arc::new(Semaphore::new(1)), second_headers.clone()]);
    let mut first = tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}));
    first["output"].as_array_mut().unwrap().insert(
        0,
        json!({
            "id":"msg_lookup","type":"message","role":"assistant","status":"completed",
            "content":[{"type":"output_text","text":"Looking up the tool","annotations":[]}]
        }),
    );
    crate::poison::mutex(&server.state.generation_replies).push_back((StatusCode::OK, first));
    *crate::poison::mutex(&server.state.generation_body) =
        tool_response("lookup", "call_client", json!({"path":"synthetic.txt"}));
    let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
    let captured = events.clone();
    let mut runtime = runtime(&server).await;
    runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
    let (base, task) = recovery::gateway(Arc::new(runtime)).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&catalog_request(true))
        .send()
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while server.state.generation_requests.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the catalog continuation must reach the held response headers");
    tokio::time::timeout(std::time::Duration::from_secs(1), closure)
        .await
        .expect("the completed body must close while the next response headers are held")
        .expect_err("the synthetic body never sends a closure value");
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    second_headers.add_permits(1);
    let body = response.text().await.unwrap();
    assert!(body.contains("Looking up the tool"));
    assert!(body.contains("\"name\":\"lookup\""));
    assert_eq!(body.matches("event: response.completed\n").count(), 1);
    let events = crate::poison::mutex(&events);
    assert_eq!(events.len(), 2);
    assert!(!events[0].success);
    assert_eq!(
        events[0].routing.as_ref().unwrap().attempt_outcome,
        Some(crate::AttemptOutcome::ToolDiscovery)
    );
    assert!(events[1].success);
    assert_eq!(events[0].input_tokens, Some(100));
    assert_eq!(events[1].input_tokens, Some(100));
    task.abort();
}

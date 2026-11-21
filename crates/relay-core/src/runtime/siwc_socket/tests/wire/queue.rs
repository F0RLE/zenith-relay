use super::*;

#[tokio::test]
async fn queued_wire_burst_preserves_every_predecessor_without_cold_sends() {
    const TURNS: usize = 12;
    let mut requests = Vec::new();
    let mut rounds = Vec::new();
    let mut history = Vec::new();
    for turn in 0..TURNS {
        history.push(json!({"role":"user","content":format!("synthetic turn {turn}")}));
        requests.push(request(json!(history)));
        let output = json!({"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":format!("synthetic answer {turn}")}]});
        rounds.push(vec![completed(
            &format!("response_{turn}"),
            json!([output]),
        )]);
        history.push(output);
    }
    let fixture = Fixture::start(rounds).await;
    let store = SocketStore::default();
    let first = fixture.pending(&store, &requests[0]).await;
    let mut waiters = Vec::new();
    for body in &requests[1..] {
        let mut waiting = Box::pin(fixture.pending(&store, body));
        assert!(waiting.as_mut().now_or_never().is_none());
        waiters.push(waiting);
    }
    assert!(fixture.requests().is_empty());
    first.send().await.unwrap().text().await.unwrap();
    for waiting in waiters {
        let response = waiting.await.send().await.unwrap();
        assert!(
            response
                .extensions()
                .get::<SocketDiagnostics>()
                .unwrap()
                .reused
        );
        response.text().await.unwrap();
    }
    let sent = fixture.requests();
    assert_eq!(sent.len(), TURNS);
    assert_eq!(crate::poison::mutex(&store.lanes).len(), 1);
    for (turn, event) in sent.iter().enumerate().skip(1) {
        assert_eq!(
            event["previous_response_id"],
            format!("response_{}", turn - 1)
        );
        assert_eq!(
            event["input"],
            json!([{"role":"user","content":format!("synthetic turn {turn}")}])
        );
    }
}

#[tokio::test]
async fn queued_turn_reuses_the_completed_socket_and_continuation() {
    let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test",
        "name":"lookup","arguments":"{}","status":"completed"});
    let fixture = Fixture::start(vec![
        vec![completed("first", json!([call]))],
        vec![completed("second", json!([]))],
    ])
    .await;
    let store = SocketStore::default();
    let first = request(json!([{"role":"user","content":"synthetic"}]));
    let owner = fixture.pending(&store, &first).await;
    let result = json!({"type":"function_call_output","call_id":"call_test","output":"ok"});
    let mut next = first.clone();
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([call, result.clone()]);
    let mut waiting = Box::pin(fixture.pending(&store, &next));
    assert!(waiting.as_mut().now_or_never().is_none());
    assert!(fixture.requests().is_empty(), "waiting must not dispatch");
    owner.send().await.unwrap().text().await.unwrap();
    let response = waiting.await.send().await.unwrap();
    assert!(
        response
            .extensions()
            .get::<SocketDiagnostics>()
            .unwrap()
            .reused
    );
    response.text().await.unwrap();
    let sent = fixture.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["previous_response_id"], "first");
    assert_eq!(sent[1]["input"], json!([result]));
}

#[tokio::test]
async fn cancelled_waiter_never_sends_or_disrupts_the_owners_continuation() {
    let output = json!({"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"synthetic reply"}]});
    let fixture = Fixture::start(vec![
        vec![completed("first", json!([output]))],
        vec![completed("second", json!([]))],
    ])
    .await;
    let store = SocketStore::default();
    let first = request(json!([{"role":"user","content":"synthetic"}]));
    let owner = fixture.pending(&store, &first).await;
    let mut waiting = Box::pin(fixture.pending(&store, &first));
    assert!(waiting.as_mut().now_or_never().is_none());
    drop(waiting);
    assert!(fixture.requests().is_empty());
    owner.send().await.unwrap().text().await.unwrap();
    let mut next = first;
    let input = json!({"role":"user","content":"next"});
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([output, input.clone()]);
    let response = fixture.pending(&store, &next).await.send().await.unwrap();
    assert!(
        response
            .extensions()
            .get::<SocketDiagnostics>()
            .unwrap()
            .reused
    );
    response.text().await.unwrap();
    let sent = fixture.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["previous_response_id"], "first");
    assert_eq!(sent[1]["input"], json!([input]));
}

use super::*;
use zenith_relay_core::AttemptOutcome;

fn outcome(event: &UsageEvent) -> Option<AttemptOutcome> {
    event
        .routing
        .as_ref()
        .and_then(|routing| routing.attempt_outcome)
}

#[tokio::test]
async fn ordinary_and_compact_account_repair_exhaustion_keep_attempt_identity_and_usage() {
    for endpoint in ["/v1/responses", "/v1/responses/compact"] {
        let rejection = |input_tokens| {
            json!({
                "error":{"message":"Missing required field: call_id"},
                "usage":{"input_tokens":input_tokens,"output_tokens":0,"total_tokens":input_tokens}
            })
        };
        let (upstream, state) = spawn_upstream(vec![
            Reply::Json(StatusCode::BAD_REQUEST, rejection(3)),
            Reply::Json(StatusCode::BAD_REQUEST, rejection(4)),
        ])
        .await;
        let authority = ready_authority("synthetic-repair-account", "synthetic-access").await;
        let (gateway, events, _, _) = spawn_mixed_gateway(
            Vec::new(),
            vec![account(
                "synthetic-repair-account",
                "synthetic-provider",
                &upstream,
                10,
            )],
            vec![mixed_key(None, None)],
            authority,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let response = reqwest::Client::new()
            .post(format!("{}{endpoint}", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL, "stream":false, "input":[
                {"type":"function_call", "name":"lookup", "arguments":"{}"},
                {"type":"function_call_output", "output":"synthetic result"}
            ]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{endpoint}");
        let _ = response.bytes().await.unwrap();
        assert_eq!(
            gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
            0
        );
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "{endpoint}");
        assert!(requests[0].body["input"][0].get("call_id").is_none());
        assert_eq!(
            requests[1].body["input"][0]["call_id"],
            requests[1].body["input"][1]["call_id"]
        );
        drop(requests);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2, "{endpoint}");
        assert_eq!(events[0].request_id, events[1].request_id);
        assert_eq!(events[0].attempt, 1);
        assert_eq!(events[1].attempt, 2);
        assert_eq!(events[0].input_tokens, Some(3));
        assert_eq!(events[1].input_tokens, Some(4));
        assert!(events.iter().all(|event| !event.success));
        assert_eq!(
            outcome(&events[0]),
            Some(AttemptOutcome::RetryRepair),
            "{endpoint}"
        );
        assert_eq!(
            outcome(&events[1]),
            Some(AttemptOutcome::ReturnedFailure),
            "{endpoint}"
        );
    }
}

#[tokio::test]
async fn compact_account_transport_failure_is_uncertain_without_another_dispatch() {
    let (upstream, state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data("data: {\"type\":\"response.in_progress\",\"response\":{\"id\":\"synthetic-partial\",\"status\":\"in_progress\",\"output\":[]}}\n\n"),
        StreamChunk::Error,
    ])]).await;
    let authority = ready_authority("synthetic-account", "synthetic-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "synthetic-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"input":"synthetic compaction"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let _ = response.bytes().await.unwrap();
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        0
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].attempt, 1);
    assert_eq!(
        outcome(&events[0]),
        Some(AttemptOutcome::ExecutionUncertain)
    );
}

#[tokio::test]
async fn compact_account_retry_selection_and_completion_share_one_logical_request() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(StatusCode::TOO_MANY_REQUESTS, json!({"error":{"code":"rate_limit_exceeded","message":"Synthetic quota limit"},"usage":{"input_tokens":7,"output_tokens":0,"total_tokens":7}})),
        Reply::Json(StatusCode::OK, compaction_body("synthetic-recovered-compaction")),
    ]).await;
    let authority = ready_authority("synthetic-account-a", "synthetic-access-a").await;
    authority
        .register(
            "synthetic-account-b",
            TokenSet::access_only("synthetic-access-b", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("synthetic-account-a", "synthetic-provider-a", &upstream, 10),
            account("synthetic-account-b", "synthetic-provider-b", &upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"input":"synthetic compaction"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.unwrap();
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    assert!(
        gateway
            .runtime
            .as_ref()
            .unwrap()
            .candidate_runtime_order()
            .iter()
            .all(|candidate| candidate.in_flight == 0)
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].request_id, events[1].request_id);
    assert_ne!(events[0].candidate_id, events[1].candidate_id);
    assert_eq!(events[0].attempt, 1);
    assert_eq!(events[1].attempt, 2);
    assert_eq!(events[0].input_tokens, Some(7));
    assert_eq!(outcome(&events[0]), Some(AttemptOutcome::RetrySelection));
    assert_eq!(outcome(&events[1]), Some(AttemptOutcome::Completed));
    assert!(!events[0].success);
    assert!(events[1].success);
}

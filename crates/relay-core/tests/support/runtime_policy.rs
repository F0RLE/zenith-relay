use super::*;
use zenith_relay_core::RuntimeCandidatePolicy;

#[tokio::test]
async fn account_policy_edits_keep_dispatched_authorization_and_fence_a_replacement_login() {
    let (upstream, state) = spawn_held_stream_upstream().await;
    let authority = ready_authority("synthetic-policy-account", "synthetic-old-access").await;
    let refresh = refresh_adapter();
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "synthetic-policy-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority.clone(),
        refresh.clone(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap();
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"stream":true,"input":"synthetic active request"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let mut active = response.bytes_stream();
    let prefix = tokio::time::timeout(Duration::from_secs(3), active.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&prefix).contains("first"));
    assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 1);
    let policy = |enabled| RuntimeCandidatePolicy {
        enabled,
        draining: false,
        priority: 4,
        weight: 2,
        allowed_models: vec![],
        excluded_models: vec![],
    };
    assert!(runtime.update_account_policy("synthetic-policy-account", policy(false)));
    assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 1);
    state.release.notify_one();
    let terminal = tokio::time::timeout(Duration::from_secs(3), async {
        let mut body = Vec::new();
        while let Some(chunk) = active.next().await {
            body.extend_from_slice(&chunk.unwrap());
        }
        body
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&terminal).contains("response.completed"));
    let candidate = &runtime.candidate_runtime_order()[0];
    assert_eq!(candidate.in_flight, 0);
    assert!(
        !candidate.available,
        "a late success must preserve operator disable"
    );
    let (old_revision, old_generation) = {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert!(events[0].account_token_generation.is_some());
        (
            events[0].account_token_revision.clone().unwrap(),
            events[0].account_token_generation,
        )
    };
    assert!(old_revision.guard().is_some());
    authority
        .register(
            "synthetic-policy-account",
            TokenSet::access_only("synthetic-new-access", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    assert!(old_revision.guard().is_none());
    assert!(runtime.update_account_policy("synthetic-policy-account", policy(true)));
    assert!(old_revision.guard().is_none());
    state.release.notify_one();
    let completed = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"stream":true,"input":"synthetic fresh request"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(completed.contains("response.completed"));
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer synthetic-old-access")
    );
    assert_eq!(
        requests[1].authorization.as_deref(),
        Some("Bearer synthetic-new-access")
    );
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
    assert_ne!(events[0].request_id, events[1].request_id);
    assert_eq!(events[1].account_token_generation, old_generation);
    let new_revision = events[1].account_token_revision.as_ref().unwrap();
    assert_ne!(&old_revision, new_revision);
    assert!(new_revision.guard().is_some());
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 0);
}

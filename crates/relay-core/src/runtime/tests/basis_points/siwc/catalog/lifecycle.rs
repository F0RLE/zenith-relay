use super::*;

#[tokio::test]
async fn siwc_historical_catalog_collision_keeps_native_forwarding() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        *crate::poison::mutex(&siwc.state.generation_body) = answer();
        let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
        let mut payload = request(stream);
        let historical = json!({"type":"additional_tools","role":"developer","tools":[{
            "type":"function","name":"relay_catalog","description":"Synthetic client-owned function",
            "parameters":{"type":"object","properties":{"path":{"type":"string"}}}
        }]});
        payload["input"]
            .as_array_mut()
            .unwrap()
            .insert(0, historical.clone());
        let reply = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&payload)
            .send()
            .await
            .unwrap();
        let status = reply.status();
        let text = reply.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(
            terminal(&text, stream)["output"][0]["content"][0]["text"],
            "Synthetic ready"
        );
        let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
        assert_eq!(bodies.len(), 1);
        let sent = &bodies[0];
        assert_eq!(sent["input"][0], historical);
        assert!(!sent.to_string().contains("Relay local tool catalog."));
        assert_eq!(sent["reasoning"], payload["reasoning"]);
        assert_eq!(
            sent["prompt_cache_options"],
            payload["prompt_cache_options"]
        );
        assert_eq!(
            crate::tool_policy::catalog_stats_with_sizes(sent, false).count,
            35
        );
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(siwc.state.attachment_requests.load(Ordering::SeqCst), 0);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_native_catalog_cancellation_does_not_start_another_generation() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    let gate = Arc::new(Semaphore::new(0));
    crate::poison::mutex(&siwc.state.stream_gates).push_back(gate.clone());
    let mut reply = lookup("resp_cancel_lookup");
    reply["output"]
        .as_array_mut()
        .unwrap()
        .insert(0, answer()["output"][0].clone());
    *crate::poison::mutex(&siwc.state.generation_body) = reply;
    let finished = Arc::new(Notify::new());
    let notified = finished.clone();
    let mut runtime = runtime(&siwc, &native).await;
    Arc::get_mut(&mut runtime).unwrap().usage = Arc::new(move |_| notified.notify_one());
    let (base, task) = recovery::gateway(runtime).await;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let mut response = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&request(true))
        .send()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut text = String::new();
        while !text.contains("Synthetic ready") {
            let chunk = response
                .chunk()
                .await
                .unwrap()
                .expect("text before terminal");
            text.push_str(std::str::from_utf8(&chunk).unwrap());
        }
    })
    .await
    .unwrap();
    drop(response);
    drop(client);
    tokio::time::timeout(Duration::from_secs(3), finished.notified())
        .await
        .unwrap();
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    gate.add_permits(1);
    task.abort();
}

#[tokio::test]
async fn siwc_native_catalog_continuation_rejects_replaced_authorization() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    let runtime = runtime(&siwc, &native).await;
    let prepared = runtime
        .prepare_authorization("synthetic-account-0", runtime_now_ms())
        .await
        .unwrap();
    let key = runtime.authenticate_secret("local-secret").unwrap();
    let route = runtime
        .configured_executor_routes(&key, "gpt-test", &[WireApi::Responses], false)
        .into_iter()
        .find(|route| route.candidate_id == "synthetic-account-0")
        .unwrap();
    runtime.account_state.executors["synthetic-account-0"]
        .token_authority
        .register(
            "synthetic-account-0",
            TokenSet::access_only("synthetic-replaced-access", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let result = runtime
        .send_account_request_with_owner(
            &route.candidate_id,
            reqwest::Client::new()
                .post(siwc.responses_url.clone())
                .json(&json!({"model":"gpt-test"})),
            AuthorizationDispatch {
                client_version: None,
                identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                turn_scope: None,
                budget: Some(&budget),
                lease: None,
                usage_event: None,
            },
            &prepared.incarnation(),
        )
        .await;
    assert!(matches!(result, Err(AuthorizedRequestError::NotReplayable)));
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
}

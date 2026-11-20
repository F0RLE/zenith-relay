use super::*;

#[tokio::test]
async fn siwc_cold_upgrade_is_inside_attempt_without_waiting_for_bps_peers() {
    const UPGRADE: Duration = Duration::from_millis(80);
    let siwc = AccessServer::start(json!([]), false).await;
    let bps = AccessServer::start(json!([{"id":"gpt-other"}]), true).await;
    *crate::poison::mutex(&siwc.state.websocket_upgrade_delay) = UPGRADE;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    let mut completed = super::json::completed_response();
    *crate::poison::mutex(&siwc.state.generation_body) = completed.clone();
    let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
    let captured = events.clone();
    let mut runtime = runtime(&siwc, &bps).await;
    let settings = Arc::get_mut(&mut runtime).unwrap();
    let bps_account = settings
        .account_state
        .executors
        .get_mut("synthetic-account-1")
        .unwrap();
    bps_account.oauth_client_kind = OAuthClientKind::ExcelBps;
    bps_account.basis_points_url = bps.responses_url.clone();
    settings.diagnostics_enabled = || true;
    settings.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
    let (base, task) = recovery::gateway(runtime.clone()).await;
    let client = retry_admission::bounded_client();
    let request = |input: Value| {
        client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .header("x-codex-session-id", "synthetic-timing-session")
            .json(&json!({"model":"gpt-test","input":input,"stream":false}))
            .send()
    };
    let initial = json!({"role":"user","content":"Synthetic initial input"});
    // BPS remains blocked until SIWC has completed its first generation.
    let first: Value = request(json!([initial]))
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], completed["id"]);
    assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 1);
    assert_eq!(bps.state.access_requests.load(Ordering::SeqCst), 1);
    bps.state.first_release.add_permits(1);
    let discovery = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.account_state.access_prefetch.lock(),
    )
    .await
    .unwrap();
    drop(discovery);
    completed["id"] = json!("resp_synthetic_warm");
    *crate::poison::mutex(&siwc.state.generation_body) = completed.clone();
    let warm: Value = request(json!([initial, first["output"][0],
        {"role":"user","content":"Synthetic next input"}]))
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(warm["id"], completed["id"]);
    assert_eq!(bps.state.access_requests.load(Ordering::SeqCst), 1);
    assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 0);
    assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 1);
    let events = crate::poison::mutex(&events);
    assert_eq!(events.len(), 2);
    for (index, event) in events.iter().enumerate() {
        let routing = event.routing.as_ref().unwrap();
        let transport = routing.transport.as_ref().unwrap();
        let timing = routing.request_timing.as_ref().unwrap();
        let phases = timing.phases.as_ref().unwrap();
        let access = phases.bps_access_refresh_ms.unwrap();
        let attempt_ttft = event.ttft_ms.unwrap();
        assert_eq!(
            phases.pre_attempt_ms + event.latency_ms + phases.post_attempt_ms.unwrap(),
            timing.duration_ms,
        );
        assert_eq!(timing.ttft_ms, Some(phases.pre_attempt_ms + attempt_ttft));
        let measured = phases.body_read_and_parse_ms.unwrap()
            + access
            + phases.cache_context_ms.unwrap()
            + phases.candidate_selection_ms.unwrap()
            + transport.request_preparation_ms.unwrap();
        assert!(measured <= phases.pre_attempt_ms);
        assert_eq!(transport.connection_reused, Some(index == 1));
        if index == 0 {
            let upgrade = transport.handshake_ms.unwrap();
            assert!(upgrade >= UPGRADE.as_millis() as u64);
            assert!(upgrade <= attempt_ttft);
        } else {
            assert_eq!(transport.handshake_ms, None);
        }
    }
    task.abort();
}

#[tokio::test]
async fn known_siwc_model_does_not_wait_for_unrelated_bps_access() {
    let siwc = AccessServer::start(json!([]), false).await;
    let bps = AccessServer::start(json!([{"id":"gpt-other"}]), true).await;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    *crate::poison::mutex(&siwc.state.generation_body) = super::json::completed_response();
    let mut runtime = runtime(&siwc, &bps).await;
    let account = Arc::get_mut(&mut runtime)
        .unwrap()
        .account_state
        .executors
        .get_mut("synthetic-account-1")
        .unwrap();
    account.oauth_client_kind = OAuthClientKind::ExcelBps;
    account.basis_points_url = bps.responses_url.clone();
    assert!(runtime.update_account_models("synthetic-account-1", &["gpt-other".to_owned()]));
    let (base, task) = recovery::gateway(runtime).await;
    let client = retry_admission::bounded_client();
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic prompt","stream":false}))
            .send(),
    )
    .await
    .expect("Unrelated access endpoint is blocked")
    .unwrap()
    .error_for_status()
    .unwrap();
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["status"], "completed");
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(bps.state.access_requests.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn siwc_generation_uses_websocket_even_when_client_websockets_are_disabled() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    *crate::poison::mutex(&siwc.state.generation_body) = json!({
        "id":"resp_socket_only","status":"completed","output":[{
            "type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic socket answer"
            }]
        }]
    });
    let runtime = runtime(&siwc, &native).await;
    runtime.set_codex_websockets_enabled(false);
    let (base, task) = recovery::gateway(runtime).await;
    let input = json!([{"role":"user","content":[
        {"type":"input_text","text":"Synthetic image question"},
        {"type":"input_image","image_url":"https://example.invalid/synthetic.png"}
    ]}]);
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":input,"stream":false}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Synthetic socket answer"), "{body}");
    assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 1);
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        crate::poison::mutex(&siwc.state.websocket_bodies)[0]["input"],
        input
    );
    assert_eq!(
        siwc.state.http_generation_requests.load(Ordering::SeqCst),
        0
    );
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    assert!(
        crate::poison::mutex(&siwc.state.websocket_bodies)
            .iter()
            .all(|body| body.get("previous_response_id").is_none())
    );
    task.abort();
}

#[tokio::test]
async fn siwc_rejected_upgrades_never_start_http_generations() {
    for code in [200, 400, 404, 405, 426, 429, 500, 501, 502, 503, 504] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        *crate::poison::mutex(&siwc.state.websocket_upgrade_reply) = Some(AccessReply {
            status: StatusCode::from_u16(code).unwrap(),
            body: json!({"error":{"code":"synthetic_handshake_rejection","message":"Synthetic handshake rejection"}}),
        });
        let runtime = runtime(&siwc, &native).await;
        let (base, task) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":false}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if code == 200 { 502 } else { code },
            "{code}"
        );
        let body: Value = response.json().await.unwrap();
        if code == 200 {
            assert_eq!(
                body["error"]["code"],
                crate::error_codes::SIWC_WEBSOCKET_REQUIRED
            );
        } else {
            assert_eq!(body["error"]["code"], "synthetic_handshake_rejection");
        }
        assert_eq!(
            siwc.state.websocket_requests.load(Ordering::SeqCst),
            1,
            "{code}"
        );
        assert_eq!(
            siwc.state.generation_requests.load(Ordering::SeqCst),
            0,
            "{code}"
        );
        assert_eq!(
            siwc.state.http_generation_requests.load(Ordering::SeqCst),
            0,
            "{code}"
        );
        assert_eq!(
            native.state.generation_requests.load(Ordering::SeqCst),
            0,
            "{code}"
        );
        task.abort();
    }
}

#[tokio::test]
async fn siwc_unsupported_endpoint_is_rejected_before_any_wire_attempt() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    let mut runtime = runtime(&siwc, &native).await;
    Arc::get_mut(&mut runtime)
        .unwrap()
        .account_state
        .executors
        .get_mut("synthetic-account-0")
        .unwrap()
        .responses_url
        .set_query(Some("unsupported=1"));
    let (base, task) = recovery::gateway(runtime).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["error"]["code"],
        crate::error_codes::SIWC_WEBSOCKET_REQUIRED
    );
    assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 0);
    assert_eq!(
        siwc.state.http_generation_requests.load(Ordering::SeqCst),
        0
    );
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}

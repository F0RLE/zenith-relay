use super::*;

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

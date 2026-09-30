use super::*;

#[tokio::test]
async fn original_errors_reach_usage_for_http_failed_json_and_sse() {
    for (status, streaming) in [(422, false), (200, false), (200, true)] {
        let payload = json!({"error": {"code": "future_validation_error", "type": "invalid_request_error", "message": "Invalid field: temperature"}});
        let body = if streaming {
            format!(
                "data: {}\n\n",
                json!({"type": "response.failed", "response": payload})
            )
        } else {
            payload.to_string()
        };
        let upstream = spawn(Router::new().route(
            "/v1/responses",
            post(move || {
                let body = body.clone();
                async move {
                    Response::builder()
                        .status(status)
                        .header(
                            CONTENT_TYPE,
                            if streaming {
                                "text/event-stream"
                            } else {
                                "application/json"
                            },
                        )
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        ))
        .await;
        let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":"gpt-test", "input":"synthetic input", "stream":streaming}))
            .send()
            .await
            .unwrap();
        let response = response.text().await.unwrap();
        assert!(response.contains("Invalid field: temperature"));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert!(!event.success);
        assert_eq!(
            event.error_category.as_deref(),
            Some("upstream_invalid_request")
        );
        assert!(event.cooldown_scope.is_none());
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(status));
        assert_eq!(details.code.as_deref(), Some("future_validation_error"));
        assert_eq!(
            details.message.as_deref(),
            Some("Invalid field: temperature")
        );
    }
}

#[tokio::test]
async fn malformed_sse_records_parser_diagnostics_before_output() {
    let upstream = spawn(Router::new().route(
        "/v1/responses",
        post(|| async {
            Response::builder()
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(
                    "event: synthetic-private\ndata: {\"synthetic-private\":\n\n",
                ))
                .unwrap()
        }),
    ))
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-test", "input":"synthetic input", "stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["error"]["code"], "stream_invalid");
    let events = events.lock().unwrap();
    assert!(!events.is_empty());
    for event in events.iter() {
        assert!(!event.success);
        assert_eq!(event.error_category.as_deref(), Some("stream_invalid"));
        assert!(event.ttft_ms.is_none());
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(200));
        assert_eq!(details.error_type.as_deref(), Some("relay_stream_parser"));
        let message = details.message.as_deref().unwrap();
        assert!(message.contains("category=Eof"));
        assert!(!message.contains("synthetic-private"));
    }
}

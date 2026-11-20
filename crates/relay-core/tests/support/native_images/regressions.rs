use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

#[tokio::test]
async fn image_request_validation_preserves_nullable_options_and_provider_urls() {
    let (upstream, state) = spawn_upstream(vec![Reply::Json(
        StatusCode::OK,
        json!({
            "data":[{"b64_json":"", "url":"https://example.invalid/generated-image"}],
            "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}
        }),
    )])
    .await;
    let mut image_source = source("image-source", &upstream, "synthetic-key", 10);
    image_source.source.models = vec![zenith_relay_core::IMAGE_API_MODEL.to_owned()];
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![image_source],
        Vec::new(),
        vec![mixed_key(None, None)],
        ready_authority("unused-account", "synthetic-access").await,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let client = reqwest::Client::new();
    for body in [
        json!({"prompt":"synthetic", "images":[{"image_url":"https://example.invalid/source","file_id":"file-synthetic"}]}),
        json!({"prompt":"synthetic", "images":[{"image_url":"https://example.invalid/source"}], "mask":{"image_url":"https://example.invalid/mask","file_id":"file-mask"}}),
        json!({"prompt":"synthetic", "images":[{"image_url":"https://example.invalid/source","file_id":42}]}),
        json!({"prompt":"synthetic", "images":[{"file_id":"file-synthetic"}]}),
    ] {
        let response = client
            .post(format!("{}/v1/images/edits", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert!(state.requests.lock().unwrap().is_empty());
    }
    for (field, value) in [
        ("model", json!(7)),
        ("model", json!({})),
        ("model", json!(false)),
        ("response_format", json!(7)),
        ("response_format", json!([])),
        ("stream", json!("true")),
        ("stream", json!({})),
        ("n", json!("1")),
        ("n", json!(0)),
        ("n", json!(11)),
        ("n", json!(1.5)),
        ("prompt", json!(" \n\t ")),
    ] {
        let mut body = json!({"prompt":"synthetic"});
        body[field] = value;
        let response = client
            .post(format!("{}/v1/images/generations", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert!(state.requests.lock().unwrap().is_empty());
    }
    let response = client.post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":null,"prompt":"synthetic","n":null,"stream":null,"response_format":"url"}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["data"][0]["url"],
        "https://example.invalid/generated-image"
    );
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["model"],
        zenith_relay_core::IMAGE_API_MODEL
    );
    assert_eq!(requests[0].body.get("n"), Some(&Value::Null));
    assert_eq!(requests[0].body.get("stream"), Some(&Value::Null));
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 1);
}

#[tokio::test]
async fn native_image_markup_uploads_preserve_binary_data_and_reject_mask_conflicts() {
    let (upstream, state) = spawn_upstream(
        (0..2)
            .map(|_| Reply::Json(StatusCode::OK, json!({"data":[{"b64_json":"aW1hZ2U="}]})))
            .collect(),
    )
    .await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "image-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        ready_authority("image-account", "synthetic-access").await,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let png = b"\x89PNG\r\n\x1a\nsynthetic";
    let image_url = format!("data:image/png;base64,{}", STANDARD.encode(png));
    for multipart in [false, true] {
        for mask in [true, false] {
            let request = reqwest::Client::new()
                .post(format!("{}/v1/images/edits", gateway.base_url))
                .bearer_auth(LOCAL_KEY);
            let request = if multipart {
                let mut body = Vec::new();
                for (name, value) in [("model", "gpt-image-2.6-flare"), ("prompt", "synthetic")] {
                    body.extend_from_slice(format!("--synthetic\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
                }
                for name in ["image", "markup"]
                    .into_iter()
                    .chain(mask.then_some("mask"))
                {
                    body.extend_from_slice(format!("--synthetic\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"synthetic.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
                    body.extend_from_slice(png);
                    body.extend_from_slice(b"\r\n");
                }
                body.extend_from_slice(b"--synthetic--\r\n");
                request
                    .header(CONTENT_TYPE, "multipart/form-data; boundary=synthetic")
                    .body(body)
            } else {
                let mut body = json!({"model":"gpt-image-2.6-flare", "prompt":"synthetic", "images":[{"image_url":image_url}], "markup":{"image_url":image_url}});
                if mask {
                    body["mask"] = json!({"image_url":image_url});
                }
                request.json(&body)
            };
            let before = state.requests.lock().unwrap().len();
            let response = request.send().await.unwrap();
            assert_eq!(
                response.status(),
                if mask {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::OK
                }
            );
            if mask {
                assert_eq!(state.requests.lock().unwrap().len(), before);
            }
        }
    }
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for sent in requests.iter() {
        assert_eq!(sent.path, "/v1/images/edits");
        assert_eq!(sent.body["model"], "gpt-image-2.6-flare");
        assert_eq!(sent.body["images"], json!([{"image_url":image_url}]));
        assert_eq!(sent.body["markup"], json!({"image_url":image_url}));
        assert!(sent.body.get("mask").is_none());
    }
}

#[tokio::test]
async fn codex_edit_stream_completion_reaches_http_client_under_edit_event_names() {
    let (upstream, state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data("event: image_generation.partial_image\ndata: {\"type\":\"image_generation.partial_image\",\"b64_json\":\"cHJldmlldw==\"}\n\n"),
        StreamChunk::Data("event: image_generation.completed\ndata: {\"type\":\"image_generation.completed\",\"b64_json\":\"aW1hZ2U=\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3,\"total_tokens\":5}}\n\n"),
    ])]).await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![account(
            "image-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        ready_authority("image-account", "synthetic-access").await,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            diagnostics_enabled: || true,
            ..Default::default()
        },
    )
    .await;
    let response = reqwest::Client::new().post(format!("{}/v1/images/edits", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-image-2.6-flare", "prompt":"synthetic", "stream":true,"images":[{"image_url":"data:image/png;base64,YQ=="}]}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("event: image_edit.partial_image"));
    assert!(body.contains("event: image_edit.completed"));
    assert!(!body.contains("image_generation."));
    assert!(!body.contains("stream_incomplete"));
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(5));
    let routing = events[0].routing.as_ref().unwrap();
    let transport = routing.transport.as_ref().unwrap();
    assert_eq!(transport.upstream_http_status, Some(200));
    assert_eq!(transport.downstream_http_status, Some(200));
    assert!(transport.response_headers_ms.is_some());
    assert!(
        transport
            .stream_response
            .as_ref()
            .unwrap()
            .upstream_output_first_ms
            .is_some()
    );
    assert!(transport.client_first_output_ms.is_some());
    assert!(transport.upstream_response_ms.is_some());
    assert!(transport.client_response_ready_ms >= transport.upstream_response_ms);
    let timing = routing.request_timing.as_ref().unwrap();
    let phases = timing.phases.as_ref().unwrap();
    assert!(phases.body_read_and_parse_ms.is_some());
    assert_eq!(
        phases.pre_attempt_ms
            + phases.prior_attempts_ms.unwrap()
            + events[0].latency_ms
            + phases.post_attempt_ms.unwrap(),
        timing.duration_ms
    );
}

#[tokio::test]
async fn svg_usage_is_preserved_on_http_success_and_failure_without_changing_wire_counters() {
    for status in [StatusCode::OK, StatusCode::BAD_REQUEST] {
        let mut payload = json!({
            "data":[{"b64_json":"aW1hZ2U="}], "output_format":"svg",
            "usage":{"input_tokens":10,"output_tokens":6,"total_tokens":16,"output_tokens_details":{"image_tokens":4,"text_tokens":2}},
            "svg_usage":{"model":"synthetic-svg-author","input_tokens":3,"output_tokens":5,"reasoning_tokens":2,"total_tokens":8}
        });
        if !status.is_success() {
            payload["error"] = json!({"message":"synthetic failure", "type":"invalid_request"});
        }
        let (upstream, _) = spawn_upstream(vec![Reply::Json(status, payload)]).await;
        let (gateway, events, _, _) = spawn_mixed_gateway(
            Vec::new(),
            vec![account(
                "image-account",
                "synthetic-provider",
                &upstream,
                10,
            )],
            vec![mixed_key(None, None)],
            ready_authority("image-account", "synthetic-access").await,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let response = reqwest::Client::new().post(format!("{}/v1/images/generations", gateway.base_url))
            .bearer_auth(LOCAL_KEY).json(&json!({"model":"gpt-image-2.6-sunburst","prompt":"synthetic","output_format":"svg"}))
            .send().await.unwrap();
        assert_eq!(response.status(), status);
        let body: Value = response.json().await.unwrap();
        if status.is_success() {
            assert_eq!(body["usage"]["total_tokens"], 16);
            assert_eq!(body["svg_usage"]["total_tokens"], 8);
        }
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.success, status.is_success());
        assert_eq!(event.input_tokens, Some(13));
        assert_eq!(event.output_tokens, Some(11));
        assert_eq!(event.total_tokens, Some(24));
        assert_eq!(event.reasoning_tokens, Some(2));
        let image = event.image_usage.as_ref().unwrap();
        assert_eq!(image.output_image_tokens, Some(4));
        assert_eq!(image.output_text_tokens, Some(2));
        assert_eq!(image.svg_usage.as_ref().unwrap().total_tokens, Some(8));
    }
}

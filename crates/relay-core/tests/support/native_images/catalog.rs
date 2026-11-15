use super::*;
use zenith_relay_core::{CODEX_IMAGE_MODELS, IMAGE_API_MODEL};

#[tokio::test]
async fn native_image_catalog_preserves_every_model_and_default_through_http() {
    let reply = || {
        Reply::Json(
            StatusCode::OK,
            json!({
                "data": [{"b64_json": "aW1hZ2U="}],
                "usage": {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7}
            }),
        )
    };
    let (upstream, state) =
        spawn_upstream((0..=CODEX_IMAGE_MODELS.len()).map(|_| reply()).collect()).await;
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
            native_codex_images_only: true,
            ..Default::default()
        },
    )
    .await;
    let client = reqwest::Client::new();
    assert_eq!(models(&gateway).await, CODEX_IMAGE_MODELS);

    let unsupported = client
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-image-unknown", "prompt":"synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unsupported.status(), StatusCode::NOT_FOUND);
    assert!(state.requests.lock().unwrap().is_empty());

    for (index, selected) in CODEX_IMAGE_MODELS
        .iter()
        .copied()
        .map(Some)
        .chain([None])
        .enumerate()
    {
        let (format, mime) = match index {
            0 => ("svg", "image/svg+xml"),
            1 => ("gif", "image/gif"),
            _ => ("png", "image/png"),
        };
        let mut body = json!({"prompt":"synthetic", "output_format":format, "response_format":"url", "complexity":"medium"});
        if let Some(model) = selected {
            body["model"] = json!(model);
        }
        if format == "gif" {
            body["frame_count"] = json!(6);
            body["frame_duration_ms"] = json!(120);
        }
        let response = client
            .post(format!("{}/v1/images/generations", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{selected:?}");
        let response: Value = response.json().await.unwrap();
        assert_eq!(
            response["data"][0]["url"],
            format!("data:{mime};base64,aW1hZ2U=")
        );
        assert_eq!(response["usage"]["total_tokens"], 7);
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), index + 1);
        let sent = &requests[index];
        assert_eq!(sent.path, "/v1/images/generations");
        assert_eq!(sent.body["model"], selected.unwrap_or(IMAGE_API_MODEL));
        assert_eq!(sent.body["output_format"], format);
        assert_eq!(sent.body["complexity"], "medium");
        if format == "gif" {
            assert_eq!(sent.body["frame_count"], 6);
            assert_eq!(sent.body["frame_duration_ms"], 120);
        }
        assert!(sent.body.get("tools").is_none());
        assert!(sent.body.get("response_format").is_none());
    }
    let events = events.lock().unwrap();
    assert_eq!(events.len(), CODEX_IMAGE_MODELS.len() + 1);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.total_tokens == Some(7))
    );
}

#[tokio::test]
async fn native_image_gif_edits_preserve_numeric_options_in_json_and_multipart() {
    let (upstream, state) = spawn_upstream(
        (0..2)
            .map(|_| {
                Reply::Json(
                    StatusCode::OK,
                    json!({"data":[{"b64_json":"aW1hZ2U="}], "output_format":"gif"}),
                )
            })
            .collect(),
    )
    .await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
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
            native_codex_images_only: true,
            ..Default::default()
        },
    )
    .await;
    let model = "gpt-image-2.6-flare";
    for multipart in [false, true] {
        let request = reqwest::Client::new()
            .post(format!("{}/v1/images/edits", gateway.base_url))
            .bearer_auth(LOCAL_KEY);
        let request = if multipart {
            let mut body = String::new();
            for (name, value) in [
                ("model", model),
                ("prompt", "synthetic"),
                ("output_format", "gif"),
                ("response_format", "url"),
                ("complexity", "low"),
                ("frame_count", "6"),
                ("frame_duration_ms", "120"),
            ] {
                body.push_str(&format!("--synthetic\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"));
            }
            body.push_str("--synthetic\r\nContent-Disposition: form-data; name=\"image\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\na\r\n--synthetic--\r\n");
            request
                .header(CONTENT_TYPE, "multipart/form-data; boundary=synthetic")
                .body(body)
        } else {
            request.json(&json!({"model":model, "prompt":"synthetic", "output_format":"gif", "response_format":"url", "complexity":"low", "frame_count":6, "frame_duration_ms":120, "images":[{"image_url":"data:image/png;base64,YQ=="}]}))
        };
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response: Value = response.json().await.unwrap();
        assert_eq!(response["data"][0]["url"], "data:image/gif;base64,aW1hZ2U=");
    }
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for sent in requests.iter() {
        assert_eq!(sent.path, "/v1/images/edits");
        assert_eq!(sent.body["model"], model);
        assert_eq!(sent.body["complexity"], "low");
        assert_eq!(sent.body["frame_count"], 6);
        assert_eq!(sent.body["frame_duration_ms"], 120);
        assert_eq!(
            sent.body["images"],
            json!([{"image_url":"data:image/png;base64,YQ=="}])
        );
    }
}

use super::*;
use zenith_relay_core::providers::chatgpt::OAuthClientKind;

#[tokio::test]
async fn native_image_edits_preserve_json_and_multipart_inputs_through_http() {
    for multipart in [false, true] {
        let (upstream, state) = spawn_upstream(vec![Reply::Json(
            StatusCode::OK,
            json!({
                "data": [{"b64_json": "aW1hZ2U="}],
                "output_format": "webp",
                "usage": {"input_tokens": 10, "output_tokens": 4, "total_tokens": 14,
                    "input_tokens_details": {"text_tokens": 3, "image_tokens": 7}}
            }),
        )])
        .await;
        let authority = ready_authority("image-account", "synthetic-access").await;
        let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
            Vec::new(),
            vec![account(
                "image-account",
                "synthetic-provider",
                &upstream,
                10,
            )],
            vec![mixed_key(None, None)],
            authority,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
            GatewayRuntimeOptions {
                native_codex_images_only: true,
                ..Default::default()
            },
        )
        .await;
        let request = reqwest::Client::new()
            .post(format!("{}/v1/images/edits", gateway.base_url))
            .bearer_auth(LOCAL_KEY);
        let request = if multipart {
            let mut body = String::new();
            for (name, value) in [
                ("model", "gpt-image-2.5-sunburst"),
                ("prompt", "synthetic edit"),
                ("quality", "low"),
                ("response_format", "url"),
                ("output_format", "webp"),
            ] {
                body.push_str(&format!(
                    "--synthetic\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                ));
            }
            for (name, filename, bytes) in [
                ("image[]", "a.png", "a"),
                ("image[]", "b.png", "b"),
                ("mask", "mask.png", "mask"),
            ] {
                body.push_str(&format!(
                    "--synthetic\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: image/png\r\n\r\n{bytes}\r\n"
                ));
            }
            body.push_str("--synthetic--\r\n");
            request
                .header(CONTENT_TYPE, "multipart/form-data; boundary=synthetic")
                .body(body)
        } else {
            request.json(&json!({
                "model": "gpt-image-2.5-sunburst", "prompt": "synthetic edit",
                "quality": "low", "response_format": "url", "output_format": "webp",
                "images": [
                    {"image_url": "data:image/png;base64,YQ=="},
                    {"image_url": "data:image/png;base64,Yg=="}
                ],
                "mask": {"image_url": "data:image/png;base64,bWFzaw=="}
            }))
        };
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response: Value = response.json().await.unwrap();
        assert_eq!(
            response["data"][0]["url"],
            "data:image/webp;base64,aW1hZ2U="
        );
        assert_eq!(response["usage"]["total_tokens"], 14);

        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/images/edits");
        assert!(requests[0].image_turn_id.is_some());
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer synthetic-access")
        );
        let body = &requests[0].body;
        assert_eq!(body["model"], "gpt-image-2.5-sunburst");
        assert_eq!(body["quality"], "low");
        assert_eq!(body["output_format"], "webp");
        assert_eq!(
            body["images"],
            json!([
                {"image_url": "data:image/png;base64,YQ=="},
                {"image_url": "data:image/png;base64,Yg=="}
            ])
        );
        assert_eq!(body["mask"]["image_url"], "data:image/png;base64,bWFzaw==");
        assert!(body.get("tools").is_none());
        assert!(body.get("response_format").is_none());
        drop(requests);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert_eq!(events[0].total_tokens, Some(14));
        assert_eq!(
            events[0].image_usage.as_ref().unwrap().input_image_tokens,
            Some(7)
        );
        assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 1);
    }
}

#[tokio::test]
async fn images_only_host_blocks_text_endpoints_and_executes_native_generation() {
    let (upstream, state) = spawn_upstream(vec![Reply::Json(
        StatusCode::OK,
        json!({"data": [{"b64_json": "aW1hZ2U="}]}),
    )])
    .await;
    let authority = ready_authority("image-account", "synthetic-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![account(
            "image-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            native_codex_images_only: true,
            ..Default::default()
        },
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap();
    let image_model = zenith_relay_core::IMAGE_API_MODEL;
    for models in [vec![MODEL.into()], vec!["gpt-refreshed".into()], Vec::new()] {
        assert!(runtime.update_account_models("image-account", &models));
        assert_eq!(
            runtime.visible_models_for_secret(LOCAL_KEY, &[WireApi::Responses], current_time_ms()),
            [image_model]
        );
    }
    let client = reqwest::Client::new();
    for model in [MODEL, image_model] {
        for (path, body) in [
            (
                "/v1/responses",
                json!({"model": model, "input": "synthetic"}),
            ),
            (
                "/v1/responses/compact",
                json!({"model": model, "input": [{"role":"user", "content":"synthetic"}]}),
            ),
            (
                "/v1/chat/completions",
                json!({"model": model, "messages": [{"role":"user", "content":"synthetic"}]}),
            ),
            (
                "/v1/messages",
                json!({"model": model, "max_tokens": 8, "messages": [{"role":"user", "content":"synthetic"}]}),
            ),
        ] {
            let response = client
                .post(format!("{}{path}", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&body)
                .send()
                .await
                .unwrap();
            assert!(!response.status().is_success(), "{path} {model}");
            let error: Value = response.json().await.unwrap();
            assert!(error.get("error").is_some(), "{path}: {error}");
            assert!(
                state.requests.lock().unwrap().is_empty(),
                "{path} must not dispatch"
            );
        }
    }
    let mut socket = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type":"response.create", "model":MODEL, "input":"synthetic"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let event = receive_websocket_json(&mut socket).await;
    assert_eq!(event["type"], "error");
    drop(socket);
    assert!(state.requests.lock().unwrap().is_empty());

    let response = client
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": image_model, "prompt": "synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"][0]["b64_json"], "aW1hZ2U=");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/images/generations");
}

#[tokio::test]
async fn native_image_routes_respect_transport_and_key_scope() {
    for (kind, deny_accounts) in [
        (OAuthClientKind::ExcelBps, false),
        (OAuthClientKind::Siwc, false),
        (OAuthClientKind::Codex, true),
    ] {
        let (upstream, state) = spawn_upstream(Vec::new()).await;
        let authority = ready_authority("image-account", "synthetic-access").await;
        let mut image_account = account("image-account", "synthetic-provider", &upstream, 10);
        image_account.oauth_client_kind = kind;
        let (gateway, _, _, _) = spawn_mixed_gateway(
            Vec::new(),
            vec![image_account],
            vec![mixed_key(None, None)],
            authority,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        if deny_accounts {
            assert!(gateway.runtime.as_ref().unwrap().update_key_scope(
                "local-key",
                CandidateScope {
                    account_ids: Some(BTreeSet::new()),
                    ..CandidateScope::default()
                },
            ));
        }
        let response = reqwest::Client::new()
            .post(format!("{}/v1/images/generations", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": "gpt-image-2.5-sunburst", "prompt": "synthetic"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{kind:?}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "model_not_found");
        assert!(state.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_image_turn_ids_are_relay_owned_and_never_sent_to_api_sources() {
    for native_account in [true, false] {
        let image_model = zenith_relay_core::IMAGE_API_MODEL;
        let (upstream, state) = spawn_upstream(vec![
            Reply::Json(StatusCode::OK, json!({"data":[{"b64_json":"aW1hZ2U="}]})),
            Reply::Json(StatusCode::OK, json!({"data":[{"b64_json":"aW1hZ2U="}]})),
        ])
        .await;
        let mut image_source = source("image-source", &upstream, "synthetic-source-key", 10);
        image_source.source.models = vec![image_model.to_string()];
        let (sources, accounts) = if native_account {
            (
                Vec::new(),
                vec![account(
                    "image-account",
                    "synthetic-provider",
                    &upstream,
                    10,
                )],
            )
        } else {
            (vec![image_source], Vec::new())
        };
        let (gateway, events, _, _) = spawn_mixed_gateway(
            sources,
            accounts,
            vec![mixed_key(None, None)],
            ready_authority("image-account", "synthetic-access").await,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        for _ in 0..2 {
            let response = reqwest::Client::new()
                .post(format!("{}/v1/images/generations", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .header("x-codex-image-turn-id", "untrusted-downstream-turn")
                .json(&json!({"model":image_model,"prompt":"synthetic"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value = response.json().await.unwrap();
            assert!(body["data"][0]["b64_json"].is_string());
        }
        let requests = state.requests.lock().unwrap();
        let events = events.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(events.len(), 2);
        for (request, event) in requests.iter().zip(events.iter()) {
            assert_eq!(request.path, "/v1/images/generations");
            if native_account {
                assert_eq!(
                    request.image_turn_id.as_deref(),
                    Some(event.request_id.as_str())
                );
                assert_ne!(
                    request.image_turn_id.as_deref(),
                    Some("untrusted-downstream-turn")
                );
            } else {
                assert!(request.image_turn_id.is_none());
            }
        }
        if native_account {
            assert_ne!(requests[0].image_turn_id, requests[1].image_turn_id);
        }
    }
}

#[tokio::test]
async fn native_image_forbidden_is_not_replayed_as_a_responses_tool_request() {
    let (upstream, state) = spawn_upstream(vec![Reply::Json(
        StatusCode::FORBIDDEN,
        json!({"error":{"message":"Forbidden","type":"permission_denied"}}),
    )])
    .await;
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
    let response = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":zenith_relay_core::IMAGE_API_MODEL,"prompt":"synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = response.json().await.unwrap();
    assert!(body.get("error").is_some());
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/images/generations");
    assert!(requests[0].image_turn_id.is_some());
    assert!(requests[0].body.get("tools").is_none());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].http_status, 403);
}

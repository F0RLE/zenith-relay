use super::*;

async fn counting_gateway(
    upstream: &TestServer,
    protocol: WireApi,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    counting_gateway_for_key(
        upstream,
        protocol,
        RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "count-key".into(),
            secret: LOCAL_KEY.into(),
        }),
    )
    .await
}

async fn counting_gateway_for_key(
    upstream: &TestServer,
    protocol: WireApi,
    key: RuntimeLocalKey,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(ProviderSource {
            id: "count-source".into(),
            name: "Synthetic counter".into(),
            base_url: format!(
                "{}/prefix/{}",
                upstream.base_url,
                if protocol == WireApi::Gemini {
                    "v1beta"
                } else {
                    "v1"
                }
            ),
            api_key: SOURCE_KEY.into(),
            proxy_url: None,
            wire_api: protocol,
            models: vec!["count-model".into()],
        })],
        vec![
            key,
            RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "baseline-key".into(),
                secret: "synthetic-unused-baseline-key".into(),
            }),
        ],
        GatewayRuntimeOptions::default(),
        Arc::new(move |event| sink.lock().unwrap().push(event)),
    )
    .unwrap();
    (spawn(gateway::router(Arc::new(runtime))).await, events)
}

#[tokio::test]
async fn native_token_counts_preserve_requests_without_generation_or_usage() {
    for protocol in [WireApi::Messages, WireApi::Gemini] {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = observed.clone();
        let path = if protocol == WireApi::Messages {
            "/prefix/v1/messages/count_tokens"
        } else {
            "/prefix/v1beta/models/count-model:countTokens"
        };
        let upstream = spawn(Router::new().route(
            path,
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                assert!(headers.get(AUTHORIZATION).is_none());
                assert_eq!(
                    headers
                        .get(if protocol == WireApi::Messages {
                            "x-api-key"
                        } else {
                            "x-goog-api-key"
                        })
                        .unwrap(),
                    SOURCE_KEY
                );
                assert!(headers.get("x-private-client-header").is_none());
                if protocol == WireApi::Messages {
                    assert_eq!(headers.get("anthropic-version").unwrap(), "2023-06-01");
                    assert_eq!(headers.get_all("anthropic-beta").iter().count(), 2);
                }
                captured.lock().unwrap().push(body);
                async move {
                    Json(if protocol == WireApi::Messages {
                        json!({"input_tokens":42,"extension":{"kept":true}})
                    } else {
                        json!({"totalTokens":42,"cachedContentTokenCount":7})
                    })
                }
            }),
        ))
        .await;
        let (relay, events) = counting_gateway(&upstream, protocol).await;
        let body = if protocol == WireApi::Messages {
            json!({"model":"count-model","system":"Synthetic system","messages":[{"role":"user","content":"Synthetic input"}],"tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"value":{"type":"string"}}}}]})
        } else {
            json!({"generateContentRequest":{"model":"models/count-model","contents":[{"role":"user","parts":[{"text":"Synthetic input"}]}],"systemInstruction":{"parts":[{"text":"Synthetic system"}]},"tools":[{"functionDeclarations":[{"name":"lookup","parameters":{"type":"object"}}]}]}})
        };
        let client_path = if protocol == WireApi::Messages {
            "/v1/messages/count_tokens"
        } else {
            "/v1beta/models/count-model:countTokens"
        };
        for _ in 0..2 {
            let result = reqwest::Client::new()
                .post(format!("{}{client_path}", relay.base_url))
                .header(
                    if protocol == WireApi::Messages {
                        "x-api-key"
                    } else {
                        "x-goog-api-key"
                    },
                    LOCAL_KEY,
                )
                .header("anthropic-beta", "synthetic-a")
                .header("anthropic-beta", "synthetic-b")
                .header("x-private-client-header", "must-not-leak")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(result.status(), StatusCode::OK);
            let result: Value = result.json().await.unwrap();
            assert_eq!(
                result[if protocol == WireApi::Messages {
                    "input_tokens"
                } else {
                    "totalTokens"
                }],
                42
            );
        }
        assert_eq!(*observed.lock().unwrap(), vec![body.clone(), body]);
        assert!(
            events.lock().unwrap().is_empty(),
            "counts must not become generation usage"
        );
    }
}

#[tokio::test]
async fn token_counts_reject_unauthorized_mismatched_and_non_native_requests() {
    let observed = Arc::new(Mutex::new(0));
    let captured = observed.clone();
    let upstream = spawn(Router::new().fallback(post(move || {
        *captured.lock().unwrap() += 1;
        async { Json(json!({"input_tokens":1})) }
    })))
    .await;
    let (relay, _) = counting_gateway(&upstream, WireApi::Gemini).await;
    let client = reqwest::Client::new();
    for (key, body, status) in [
        (
            "wrong-key",
            json!({"contents":[]}),
            StatusCode::UNAUTHORIZED,
        ),
        (
            LOCAL_KEY,
            json!({"generateContentRequest":{"model":"models/other-model","contents":[]}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            LOCAL_KEY,
            json!({"generateContentRequest":false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            LOCAL_KEY,
            json!({"contents":[],"generateContentRequest":{"contents":[]}}),
            StatusCode::BAD_REQUEST,
        ),
        (
            LOCAL_KEY,
            json!({"contents":[],"stream":true}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = client
            .post(format!(
                "{}/v1beta/models/count-model:countTokens",
                relay.base_url
            ))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let (foreign, _) = counting_gateway(&upstream, WireApi::Responses).await;
    let response = client.post(format!("{}/v1/messages/count_tokens", foreign.base_url)).bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"count-model","messages":[{"role":"user","content":"Synthetic input"}]})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(response.json::<Value>().await.unwrap()["type"], "error");
    assert_eq!(*observed.lock().unwrap(), 0);
}

#[tokio::test]
async fn token_count_errors_are_redacted_and_invalid_counts_never_become_zero() {
    for (status, body, expected) in [
        (StatusCode::OK, json!({}), StatusCode::BAD_GATEWAY),
        (
            StatusCode::OK,
            json!({"input_tokens":-1}),
            StatusCode::BAD_GATEWAY,
        ),
        (
            StatusCode::OK,
            json!({"input_tokens":"42"}),
            StatusCode::BAD_GATEWAY,
        ),
        (
            StatusCode::OK,
            json!({"input_tokens":42,"oversized":"x".repeat(256*1024)}),
            StatusCode::BAD_GATEWAY,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"message":"private synthetic body"}}),
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            StatusCode::NOT_FOUND,
            json!({"secret":"private synthetic body"}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let upstream = spawn(Router::new().route(
            "/prefix/v1/messages/count_tokens",
            post(move || {
                let body = body.clone();
                async move { (status, [("retry-after", "12")], Json(body)) }
            }),
        ))
        .await;
        let (relay, events) = counting_gateway(&upstream, WireApi::Messages).await;
        let response = reqwest::Client::new().post(format!("{}/v1/messages/count_tokens", relay.base_url)).bearer_auth(LOCAL_KEY)
            .json(&json!({"model":"count-model","messages":[{"role":"user","content":"Synthetic input"}]})).send().await.unwrap();
        assert_eq!(response.status(), expected);
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(response.headers().get("retry-after").unwrap(), "12");
        }
        let body = response.text().await.unwrap();
        assert!(!body.contains("private synthetic body"));
        assert!(!body.contains(SOURCE_KEY));
        assert!(events.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn token_counting_honors_key_model_source_scopes_and_public_prefixes() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let captured = observed.clone();
    let upstream = spawn(Router::new().route(
        "/prefix/v1/messages/count_tokens",
        post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body);
            async { Json(json!({"input_tokens":0})) }
        }),
    ))
    .await;
    let client = reqwest::Client::new();
    for scenario in 0..4 {
        let mut key = RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "count-key".into(),
            secret: LOCAL_KEY.into(),
        });
        match scenario {
            0 => key.source_ids = Some(vec!["other-source".into()]),
            1 => key.allowed_models = vec!["other-model".into()],
            2 => key.excluded_models = vec!["count-model".into()],
            _ => key.model_prefix = Some("personal/".into()),
        }
        let (relay, events) = counting_gateway_for_key(&upstream, WireApi::Messages, key).await;
        let model = if scenario == 3 {
            "personal/count-model"
        } else {
            "count-model"
        };
        let response = client
            .post(format!("{}/v1/messages/count_tokens", relay.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":model,"messages":[{"role":"user","content":"Synthetic input"}]}))
            .send()
            .await
            .unwrap();
        if scenario == 3 {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.json::<Value>().await.unwrap()["input_tokens"], 0);
            assert_eq!(observed.lock().unwrap()[0]["model"], "count-model");
        } else {
            assert!(!response.status().is_success());
            assert!(observed.lock().unwrap().is_empty());
        }
        assert!(events.lock().unwrap().is_empty());
    }
    assert_eq!(observed.lock().unwrap().len(), 1);
}

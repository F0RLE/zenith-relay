use super::*;

const MODEL: &str = "synthetic-catalog-model";

#[tokio::test]
async fn discovered_public_endpoint_paths_select_native_before_conversion() {
    for streaming in [false, true] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let catalog_requests = requests.clone();
        let generation_requests = requests.clone();
        let observed_bodies = Arc::new(Mutex::new(Vec::new()));
        let captured_bodies = observed_bodies.clone();
        let app = Router::new()
            .route(
                "/api/v1/models",
                get(move |headers: HeaderMap| {
                    assert_eq!(headers[AUTHORIZATION], format!("Bearer {SOURCE_KEY}"));
                    catalog_requests
                        .lock()
                        .unwrap()
                        .push("/api/v1/models".to_string());
                    async { Json(json!({"data":[{"id":MODEL,"endpoints":["/api/v1/responses"]}]})) }
                }),
            )
            .fallback(post(
                move |uri: axum::http::Uri, headers: HeaderMap, Json(body): Json<Value>| {
                    assert_eq!(headers[AUTHORIZATION], format!("Bearer {SOURCE_KEY}"));
                    generation_requests
                        .lock()
                        .unwrap()
                        .push(uri.path().to_string());
                    captured_bodies.lock().unwrap().push(body.clone());
                    async move {
                        if uri.path() == "/api/v1/responses" {
                            native_reply(&body)
                        } else {
                            StatusCode::BAD_REQUEST.into_response()
                        }
                    }
                },
            ));
        let upstream = spawn(app).await;
        let mut source = RuntimeSource::unrestricted(ProviderSource {
            id: "synthetic-source".into(),
            name: "Synthetic catalog source".into(),
            base_url: format!("{}/api/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            proxy_url: None,
            wire_api: WireApi::ChatCompletions,
            models: Vec::new(),
        });
        let discovery =
            discover_source_with_protocol_config(&source.source, &[], &source.protocol_config)
                .await
                .unwrap();
        assert_eq!(requests.lock().unwrap().as_slice(), ["/api/v1/models"]);
        discovery.apply_catalog(
            &mut source.source.base_url,
            &mut source.source.models,
            &mut source.protocol_bindings,
            &mut source.protocol_config,
            &mut Default::default(),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = events.clone();
        let runtime = GatewayRuntime::from_pool(
            vec![source],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "synthetic-local-key".into(),
                secret: LOCAL_KEY.into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(move |event| captured_events.lock().unwrap().push(event)),
        )
        .unwrap();
        let gateway = spawn(gateway::router(Arc::new(runtime))).await;
        let request = json!({"model":MODEL,"input":"Synthetic request","stream":streaming,
            "tools":[{"type":"web_search"}],"synthetic_native_extension":{"opaque":"synthetic-state"}});
        let reply = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&request)
            .send()
            .await
            .unwrap();
        let status = reply.status();
        let body = reply.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("synthetic-response"));
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            ["/api/v1/models", "/api/v1/responses"]
        );
        let bodies = observed_bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["input"], request["input"]);
        assert_eq!(bodies[0]["tools"], request["tools"]);
        assert_eq!(
            bodies[0]["synthetic_native_extension"],
            request["synthetic_native_extension"]
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert_eq!(events[0].attempt, 1);
    }
}

fn native_reply(body: &Value) -> Response<Body> {
    let response = json!({"id":"synthetic-response","model":MODEL,"status":"completed","output":[],
        "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}});
    if body["stream"] == true {
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "data: {}\n\n",
                json!({"type":"response.completed","response":response})
            )))
            .unwrap()
    } else {
        Json(response).into_response()
    }
}

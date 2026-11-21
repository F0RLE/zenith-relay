use super::*;
use axum::body::Bytes;

struct ImageRequest {
    path: String,
    headers: HeaderMap,
    body: Bytes,
}

struct ImageServer {
    url: Url,
    requests: Arc<Mutex<Vec<ImageRequest>>>,
    reply: Arc<Mutex<(StatusCode, Value)>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ImageServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ImageServer {
    async fn start() -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let reply = Arc::new(Mutex::new((
            StatusCode::OK,
            json!({"data":[{"b64_json":"aW1hZ2U="}],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}),
        )));
        let captured = requests.clone();
        let response = reply.clone();
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let captured = captured.clone();
            let response = response.clone();
            async move {
                let image = matches!(uri.path(), "/v1/images/generations" | "/v1/images/edits");
                crate::poison::mutex(&captured).push(ImageRequest {
                    path: uri.path().to_owned(),
                    headers,
                    body,
                });
                if !image {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({"error":{"code":"unexpected_text_endpoint"}})),
                    );
                }
                let (status, body) = crate::poison::mutex(&response).clone();
                (status, Json(body))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/v1/responses",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            url,
            requests,
            reply,
            task,
        }
    }

    async fn runtime(&self) -> GatewayRuntime {
        let mut runtime = quota_runtime(QuotaSnapshot::default());
        let account = runtime
            .account_state
            .executors
            .get_mut("account-1")
            .unwrap();
        account.oauth_client_kind = OAuthClientKind::ExcelBps;
        account.basis_points_url = self.url.clone();
        account.responses_url = self.url.join("wrong-codex-responses").unwrap();
        account
            .token_authority
            .register(
                "account-1",
                TokenSet::access_only("synthetic-access", None, 1).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        assert!(runtime.update_account_models("account-1", &["gpt-test".into()]));
        runtime
    }
}

#[tokio::test]
async fn bps_native_images_forward_all_ids_without_responses_access_or_model_substitution() {
    let server = ImageServer::start().await;
    let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
    let captured = events.clone();
    let mut runtime = server.runtime().await;
    runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
    let runtime = Arc::new(runtime);
    let (base, task) = recovery::gateway(runtime.clone()).await;
    let selected = crate::account_image_models(OAuthClientKind::ExcelBps)
        .chain(["gpt-image-future-synthetic", "dall-e-synthetic"])
        .collect::<Vec<_>>();
    for model in &selected {
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/images/generations"))
            .bearer_auth("local-secret")
            .json(&json!({"model":model,"prompt":"  synthetic portrait\n","stream":false,"future_option":{"keep":true}}))
            .send().await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "synthetic image request must succeed"
        );
    }
    task.abort();
    let requests = crate::poison::mutex(&server.requests);
    assert_eq!(requests.len(), selected.len());
    for (request, model) in requests.iter().zip(&selected) {
        assert_eq!(request.path, "/v1/images/generations");
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            "Bearer synthetic-access"
        );
        assert!(request.headers.get("x-codex-image-turn-id").is_none());
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(
            body["model"] == *model,
            "image request must preserve the selected model"
        );
        assert_eq!(body["prompt"], "  synthetic portrait\n");
        assert_eq!(body["future_option"]["keep"], true);
        assert!(body.get("stream").is_none());
    }
    let events = crate::poison::mutex(&events);
    assert_eq!(events.len(), selected.len());
    assert!(events.iter().all(|event| {
        event.success
            && event.total_tokens == Some(5)
            && event
                .image_usage
                .as_ref()
                .is_some_and(|usage| usage.generated_images == 1)
    }));
}

#[tokio::test]
async fn bps_native_images_preserve_provider_rejection_without_replay() {
    let server = ImageServer::start().await;
    *crate::poison::mutex(&server.reply) = (
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({"error":{"code":"unsupported_model","message":"Synthetic model refusal"}}),
    );
    let (base, task) = recovery::gateway(Arc::new(server.runtime().await)).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/images/generations"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-image-synthetic-unsupported","prompt":"synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unsupported_model");
    assert_eq!(crate::poison::mutex(&server.requests).len(), 1);
    task.abort();
}

#[tokio::test]
async fn bps_native_images_keep_key_scope_and_hidden_model_permissions() {
    for hidden in [false, true] {
        let server = ImageServer::start().await;
        let runtime = Arc::new(server.runtime().await);
        let model = "gpt-image-future-synthetic";
        if hidden {
            runtime.set_hidden_models(vec![model.into()]);
        } else {
            assert!(runtime.update_key_scope(
                "key-1",
                crate::CandidateScope {
                    model_rules: crate::ModelRules {
                        allowed: ["gpt-image-2".into()].into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            ));
        }
        let (base, task) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/images/generations"))
            .bearer_auth("local-secret")
            .json(&json!({"model":model,"prompt":"synthetic"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(crate::poison::mutex(&server.requests).is_empty());
        task.abort();
    }
}

#[tokio::test]
async fn bps_native_image_capability_refusal_does_not_block_account_authorization() {
    let server = ImageServer::start().await;
    *crate::poison::mutex(&server.reply) = (
        StatusCode::FORBIDDEN,
        json!({"error":{"code":"image_generation_not_enabled","message":"Synthetic image entitlement refusal"}}),
    );
    let runtime = Arc::new(server.runtime().await);
    let (base, task) = recovery::gateway(runtime.clone()).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/images/generations"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-image-future-synthetic","prompt":"synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(crate::poison::mutex(&server.requests).len(), 1);
    assert_eq!(
        runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .health,
        CandidateHealth::Healthy
    );
    task.abort();
}

#[tokio::test]
async fn bps_native_images_edits_encode_uploads_without_codex_json_rewrite() {
    let server = ImageServer::start().await;
    let (base, task) = recovery::gateway(Arc::new(server.runtime().await)).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/images/edits"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-image-2.6-flare","prompt":"  synthetic edit\n","stream":false,
            "images":[{"image_url":"data:image/png;base64,YQ=="},{"image_url":"data:image/png;base64,Yg=="}],
            "mask":{"image_url":"data:image/png;base64,bWFzaw=="},"future_option":"preserved"}))
        .send().await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await.unwrap()
    );
    task.abort();
    let (headers, body) = {
        let requests = crate::poison::mutex(&server.requests);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/images/edits");
        (requests[0].headers.clone(), requests[0].body.clone())
    };
    let boundary = multer::parse_boundary(headers["content-type"].to_str().unwrap()).unwrap();
    let mut parts = multer::Multipart::new(
        futures_util::stream::once(std::future::ready(Ok::<_, std::io::Error>(body))),
        boundary,
    );
    let mut values = Vec::new();
    while let Some(field) = parts.next_field().await.unwrap() {
        let name = field.name().unwrap().to_owned();
        values.push((name, field.bytes().await.unwrap().to_vec()));
    }
    assert!(values.contains(&("model".into(), b"gpt-image-2.6-flare".to_vec())));
    assert!(values.contains(&("prompt".into(), b"  synthetic edit\n".to_vec())));
    assert_eq!(
        values
            .iter()
            .filter(|(name, _)| name == "image[]")
            .map(|(_, bytes)| bytes.as_slice())
            .collect::<Vec<_>>(),
        vec![b"a".as_slice(), b"b".as_slice()]
    );
    assert!(values.contains(&("mask".into(), b"mask".to_vec())));
    assert!(values.contains(&("future_option".into(), b"preserved".to_vec())));
    assert!(
        !values
            .iter()
            .any(|(name, _)| matches!(name.as_str(), "stream" | "images"))
    );
}

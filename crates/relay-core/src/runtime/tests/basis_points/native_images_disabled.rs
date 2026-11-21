use super::*;

async fn fixture() -> (
    Arc<GatewayRuntime>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = requests.clone();
    let server = Router::new().fallback(move |uri: Uri| {
        let access = uri.path().ends_with("/access");
        if !access {
            captured.fetch_add(1, Ordering::SeqCst);
        }
        async move {
            if access {
                (
                    StatusCode::OK,
                    Json(json!({"allowed":true,"model_catalog":{"models":[{"id":"gpt-test"}]}})),
                )
            } else {
                (StatusCode::IM_A_TEAPOT, Json(json!({})))
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!(
        "http://{}/v1/responses",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    let mut runtime = quota_runtime(QuotaSnapshot::default());
    let account = runtime
        .account_state
        .executors
        .get_mut("account-1")
        .unwrap();
    account.oauth_client_kind = OAuthClientKind::ExcelBps;
    account.basis_points_url = endpoint.clone();
    account.responses_url = endpoint;
    account
        .token_authority
        .register(
            "account-1",
            TokenSet::access_only("synthetic-access", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    assert!(runtime.update_account_models(
        "account-1",
        &[
            "gpt-test".into(),
            "gpt-image-2".into(),
            "dall-e-synthetic".into()
        ]
    ));
    runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    (Arc::new(runtime), requests, task)
}

#[tokio::test]
async fn disabled_bps_images_are_not_advertised_or_dispatched_even_from_saved_inventory() {
    let (runtime, requests, upstream) = fixture().await;
    let key = runtime.authenticate_secret("local-secret").unwrap();
    assert_eq!(
        runtime.visible_models(&key, &[WireApi::Responses], crate::unix_time_ms()),
        ["gpt-test"]
    );
    let (base, gateway) = recovery::gateway(runtime.clone()).await;
    for model in crate::CODEX_IMAGE_MODELS.iter().copied().chain([
        "gpt-image-2",
        "gpt-image-future-synthetic",
        "dall-e-synthetic",
    ]) {
        assert!(
            runtime
                .image_executor_route(
                    "account-1",
                    model,
                    &Default::default(),
                    &[WireApi::Responses]
                )
                .is_none()
        );
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/images/generations"))
            .bearer_auth("local-secret")
            .json(&json!({"model":model,"prompt":"synthetic"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "image_generation_not_enabled");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("upstream provider")
        );
    }
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    gateway.abort();
    upstream.abort();
}

#[tokio::test]
async fn disabled_bps_image_explanation_does_not_bypass_hidden_models_or_key_permissions() {
    for hidden in [false, true] {
        let (runtime, requests, upstream) = fixture().await;
        if hidden {
            runtime.set_hidden_models(vec!["gpt-image-2".into()]);
        } else {
            assert!(runtime.update_key_scope(
                "key-1",
                crate::CandidateScope {
                    model_rules: crate::ModelRules {
                        allowed: ["gpt-test".into()].into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            ));
        }
        let (base, gateway) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/images/generations"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-image-2","prompt":"synthetic"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        gateway.abort();
        upstream.abort();
    }
}

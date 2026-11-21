use super::*;

fn runtime() -> GatewayRuntime {
    let mut runtime = quota_runtime(QuotaSnapshot::default());
    runtime
        .account_state
        .executors
        .get_mut("account-1")
        .unwrap()
        .responses_url = Url::parse("http://127.0.0.1:9/v1/responses").unwrap();
    runtime
}

#[tokio::test]
async fn free_codex_images_are_hidden_and_rejected_locally_without_disabling_text() {
    for plan in ["free", "Free plan", " chatgpt_free_tier "] {
        let runtime = Arc::new(runtime());
        assert!(runtime.update_account_models(
            "account-1",
            &["gpt-test".into(), "gpt-image-synthetic".into()]
        ));
        assert!(runtime.update_account_subscription_plan("account-1", Some(plan)));
        let key = runtime.authenticate_secret("local-secret").unwrap();
        assert_eq!(
            runtime.visible_models(&key, &[WireApi::Responses], crate::unix_time_ms()),
            ["gpt-test"]
        );
        let (base, gateway) = recovery::gateway(runtime.clone()).await;
        for model in crate::CODEX_IMAGE_MODELS
            .iter()
            .copied()
            .chain(["gpt-image-synthetic", "dall-e-synthetic"])
        {
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
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["error"]["code"], "image_generation_not_enabled");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("OpenAI provider")
            );
        }
        gateway.abort();
    }
}

#[test]
fn codex_plan_updates_restore_images_without_restoring_an_obsolete_inventory() {
    let runtime = runtime();
    let key = runtime.authenticate_secret("local-secret").unwrap();
    assert!(runtime.update_account_subscription_plan("account-1", Some("free")));
    assert!(runtime.update_account_models("account-1", &["gpt-new".into()]));
    for plan in [Some("plus"), Some("pro"), None] {
        assert!(runtime.update_account_subscription_plan("account-1", plan));
        let models = runtime.visible_models(&key, &[WireApi::Responses], crate::unix_time_ms());
        assert!(models.contains(&"gpt-new".to_string()));
        assert!(!models.contains(&"gpt-test".to_string()));
        for image in crate::CODEX_IMAGE_MODELS {
            assert!(models.iter().any(|model| model == image));
        }
        assert!(runtime.update_account_subscription_plan("account-1", Some("free")));
    }
}

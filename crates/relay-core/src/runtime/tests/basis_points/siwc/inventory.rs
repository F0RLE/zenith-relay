use super::*;

#[tokio::test]
async fn siwc_empty_catalog_withdraws_executor_routes_without_changing_other_accounts() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    let runtime = runtime(&siwc, &native).await;
    let key = runtime.authenticate_secret("local-secret").unwrap();
    let has_siwc_route = || {
        runtime
            .configured_executor_routes(&key, "gpt-test", &[WireApi::Responses], false)
            .iter()
            .any(|route| route.candidate_id == "synthetic-account-0")
    };
    let native_models = runtime
        .lock_scheduler()
        .candidate("synthetic-account-1")
        .unwrap()
        .models
        .clone();
    assert!(has_siwc_route());

    assert!(runtime.update_account_models("synthetic-account-0", &[]));

    assert!(!has_siwc_route());
    let account = &runtime.chatgpt_accounts["synthetic-account-0"];
    let inventory = crate::poison::read(&account.model_inventory);
    assert!(inventory.configured_models.is_empty());
    assert!(inventory.image_main_model.is_none());
    drop(inventory);
    {
        let scheduler = runtime.lock_scheduler();
        assert!(
            scheduler
                .candidate("synthetic-account-0")
                .unwrap()
                .models
                .is_empty()
        );
        assert_eq!(
            scheduler.candidate("synthetic-account-1").unwrap().models,
            native_models
        );
    }

    assert!(runtime.update_account_models("synthetic-account-0", &["gpt-test".into()]));
    assert!(has_siwc_route());
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
}

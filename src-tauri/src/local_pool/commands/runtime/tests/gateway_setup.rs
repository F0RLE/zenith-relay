use super::*;

#[test]
fn persisted_last_used_timestamp_maps_to_epoch_milliseconds() {
    assert_eq!(timestamp_ms("1970-01-01T00:00:00.001Z"), Some(1));
    assert_eq!(timestamp_ms("not-a-date"), None);
}

#[test]
fn profile_recovery_error_does_not_block_the_api_gateway_reserve_setup() {
    let root =
        std::env::temp_dir().join(format!("zenith-runtime-profile-{}", uuid::Uuid::new_v4()));
    let profile = root.join("profile");
    let recovery = root.join("recovery");
    fs::create_dir_all(&profile).unwrap();
    fs::create_dir_all(&recovery).unwrap();
    fs::write(recovery.join("codex-default.json"), "invalid backup").unwrap();

    assert_eq!(
        managed_chatgpt_account_id_for_reserve(&profile, &recovery),
        None
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn exhausted_account_stays_configured_while_the_scheduler_blocks_requests() {
    assert!(account_candidate_enabled(
        true,
        Some(AccountRoutingBlockReason::QuotaExhausted)
    ));
    assert!(account_candidate_enabled(true, None));
    assert!(!account_candidate_enabled(
        true,
        Some(AccountRoutingBlockReason::ReauthRequired)
    ));
    for reason in [
        AccountRoutingBlockReason::AuthError,
        AccountRoutingBlockReason::Checkpoint,
        AccountRoutingBlockReason::Captcha,
        AccountRoutingBlockReason::SubscriptionForbidden,
        AccountRoutingBlockReason::SubscriptionExpired,
        AccountRoutingBlockReason::AccountUnhealthy,
    ] {
        assert!(!account_candidate_enabled(true, Some(reason)));
    }
    assert!(!account_candidate_enabled(
        false,
        Some(AccountRoutingBlockReason::QuotaExhausted)
    ));
}

#[tokio::test]
async fn runtime_creates_and_reuses_the_system_gateway_key() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-system-key-{id}"));
    let source_secret_ref = format!("source:system-key-{id}");
    let state = DesktopState::open(root.clone()).unwrap();
    secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
    state
        .store()
        .unwrap()
        .upsert_source(ProviderSourceRecord {
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "http://127.0.0.1:9/v1".into(),
            secret_ref: source_secret_ref.clone(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: zenith_relay_core::WireApi::Responses,
            protocol_config: Default::default(),
            protocol_bindings: Vec::new(),
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: Default::default(),
            detected_model_prices: Default::default(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        })
        .unwrap();

    let runtime = runtime_from_store(&state).await.unwrap();
    let key = state.store().unwrap().keys()[0].clone();
    let secret = secret_store::load(&key.secret_ref).unwrap().unwrap();
    assert!(key.system);
    assert!(key.enabled);
    assert!(secret.starts_with("zlr_"));

    runtime_from_store(&state).await.unwrap();
    let reused = state.store().unwrap().keys()[0].clone();
    assert_eq!(reused.id, key.id);
    assert_eq!(
        secret_store::load(&reused.secret_ref).unwrap().as_deref(),
        Some(secret.as_str())
    );

    let address = state.gateway.start(runtime, 0).await.unwrap();
    let response = reqwest::Client::new()
        .get(format!("http://{address}/v1/models"))
        .bearer_auth(&secret)
        .header(reqwest::header::CONNECTION, "close")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    // Drain our own response before shutting down. An unread body can keep
    // the connection task (and its runtime/store handles) alive on Windows.
    assert!(response.text().await.unwrap().contains("gpt-test"));

    state.gateway.stop().await;
    secret_store::delete(&source_secret_ref).unwrap();
    secret_store::delete(&key.secret_ref).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn invalid_source_is_quarantined_without_blocking_other_routes() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-source-quarantine-{id}"));
    let valid_secret_ref = format!("source:valid-quarantine-{id}");
    let invalid_secret_ref = format!("source:invalid-quarantine-{id}");
    let state = DesktopState::open(root.clone()).unwrap();
    secret_store::save(&valid_secret_ref, "valid-upstream-secret").unwrap();
    secret_store::save(&invalid_secret_ref, "invalid-upstream-secret").unwrap();
    let source = |id: &str, secret_ref: String, model: &str| ProviderSourceRecord {
        id: id.into(),
        name: id.into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "http://127.0.0.1:9/v1".into(),
        secret_ref,
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec![model.into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: Default::default(),
        detected_model_prices: Default::default(),
        last_used_at: None,
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    };
    let valid = source("valid_source", valid_secret_ref.clone(), "gpt-valid");
    let mut invalid = source("invalid_source", invalid_secret_ref.clone(), "gpt-invalid");
    invalid.base_url = "not-a-url".into();
    state
        .store()
        .unwrap()
        .replace_records(vec![valid, invalid], Vec::new())
        .unwrap();

    let runtime = runtime_from_store(&state).await.unwrap();
    let order = runtime.candidate_runtime_order();
    assert!(order
        .iter()
        .any(|candidate| candidate.candidate_id == "valid_source"));
    assert!(!order
        .iter()
        .any(|candidate| candidate.candidate_id == "invalid_source"));
    assert_eq!(
        state
            .store()
            .unwrap()
            .source("invalid_source")
            .and_then(|source| source.last_error.as_deref()),
        Some("source_runtime_invalid")
    );

    secret_store::delete(&valid_secret_ref).unwrap();
    secret_store::delete(&invalid_secret_ref).unwrap();
    let key_secret_ref = state
        .store()
        .unwrap()
        .keys()
        .iter()
        .find(|key| key.system)
        .map(|key| key.secret_ref.clone());
    if let Some(secret_ref) = key_secret_ref {
        secret_store::delete(&secret_ref).unwrap();
    }
    drop(runtime);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

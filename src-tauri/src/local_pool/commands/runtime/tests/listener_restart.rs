use super::*;

#[tokio::test]
async fn runtime_restarts_after_pool_eviction_and_source_deletion() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-empty-pool-{id}"));
    let source_secret_ref = format!("source:empty-pool-{id}");
    let state = DesktopState::open(root.clone()).unwrap();
    secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
    let source = ProviderSourceRecord {
        id: "source_1".into(),
        name: "Synthetic".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "http://127.0.0.1:9/v1".into(),
        secret_ref: source_secret_ref.clone(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
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
    };
    state
        .store()
        .unwrap()
        .upsert_source(source.clone())
        .unwrap();

    let runtime = runtime_from_store(&state).await.unwrap();
    let key = state
        .store()
        .unwrap()
        .keys()
        .iter()
        .find(|key| key.system)
        .cloned()
        .unwrap();
    let secret = secret_store::load(&key.secret_ref).unwrap().unwrap();
    let address = state.gateway.start(runtime, 0).await.unwrap();
    let previous_runtime = state.gateway.runtime().await.unwrap();
    assert!(previous_runtime
        .candidate_runtime_order_for_key(&key.id)
        .iter()
        .any(|candidate| candidate.available));
    let mut gateway = state.store().unwrap().gateway().clone();
    gateway.port = address.port();
    state.store().unwrap().replace_gateway(gateway).unwrap();
    let client = reqwest::Client::new();
    let initial_models: serde_json::Value = client
        .get(format!("http://{address}/v1/models"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(initial_models["data"].as_array().unwrap().len(), 1);

    let mut outside_pool = source;
    outside_pool.in_pool = false;
    let (old_sources, keys) = {
        let store = state.store().unwrap();
        (store.sources().to_vec(), store.keys().to_vec())
    };
    state
        .store()
        .unwrap()
        .replace_records(vec![outside_pool], keys.clone())
        .unwrap();
    restart_or_rollback(&state, || {
        state.store()?.replace_records(old_sources, keys.clone())
    })
    .await
    .unwrap();
    assert_eq!(state.gateway.address().await, Some(address));
    assert!(previous_runtime
        .candidate_runtime_order_for_key(&key.id)
        .iter()
        .all(|candidate| !candidate.available));
    drop(previous_runtime);
    let client = reqwest::Client::new();
    let evicted_models: serde_json::Value = client
        .get(format!("http://{address}/v1/models"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(evicted_models["data"].as_array().unwrap().is_empty());

    let (old_sources, old_keys) = {
        let store = state.store().unwrap();
        (store.sources().to_vec(), store.keys().to_vec())
    };
    state
        .store()
        .unwrap()
        .replace_records(Vec::new(), old_keys.clone())
        .unwrap();
    restart_or_rollback(&state, || {
        state.store()?.replace_records(old_sources, old_keys)
    })
    .await
    .unwrap();
    assert_eq!(state.gateway.address().await, Some(address));
    let client = reqwest::Client::new();
    let deleted_models: serde_json::Value = client
        .get(format!("http://{address}/v1/models"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(deleted_models["data"].as_array().unwrap().is_empty());

    state.gateway.stop().await;
    let restarted_runtime = runtime_from_store(&state).await.unwrap();
    assert!(restarted_runtime
        .visible_models_for_secret(&secret, &[WireApi::Responses], current_time_ms())
        .is_empty());
    drop(restarted_runtime);
    secret_store::delete(&source_secret_ref).unwrap();
    secret_store::delete(&key.secret_ref).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn runtime_repairs_missing_enabled_gateway_key_secret() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-key-repair-{id}"));
    let source_secret_ref = format!("source:key-repair-{id}");
    let key_secret_ref = format!("key:key-repair-{id}");
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
    state
        .store()
        .unwrap()
        .upsert_key(LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Default".into(),
            enabled: true,
            system: true,
            secret_ref: key_secret_ref.clone(),
            created_at: "2026-07-15T00:00:00Z".into(),
            last_used_at: None,
        })
        .unwrap();

    let runtime = runtime_from_store(&state).await.unwrap();
    let generated = secret_store::load(&key_secret_ref).unwrap().unwrap();
    assert!(generated.starts_with("zlr_"));
    let address = state.gateway.start(runtime, 0).await.unwrap();
    let response = reqwest::Client::new()
        .get(format!("http://{address}/v1/models"))
        .bearer_auth(&generated)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());

    state.gateway.stop().await;
    secret_store::delete(&source_secret_ref).unwrap();
    secret_store::delete(&key_secret_ref).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn invalid_source_start_keeps_the_remaining_pool_route_available() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-source-restart-{id}"));
    let source_secret_ref = format!("source:source-restart-{id}");
    let invalid_secret_ref = format!("source:invalid-restart-{id}");
    let key_secret_ref = format!("key:source-restart-{id}");
    let state = DesktopState::open(root.clone()).unwrap();
    secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
    secret_store::save(&invalid_secret_ref, "invalid-upstream-secret").unwrap();
    secret_store::save(&key_secret_ref, "old-secret").unwrap();
    let source = ProviderSourceRecord {
        id: "old_source".into(),
        name: "Old".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "http://127.0.0.1:9/v1".into(),
        secret_ref: source_secret_ref.clone(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["old-model".into()],
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
    let key = LocalGatewayKeyRecord {
        id: "old_key".into(),
        label: "Old key".into(),
        enabled: true,
        system: true,
        secret_ref: key_secret_ref.clone(),
        created_at: "2026-08-05T00:00:00Z".into(),
        last_used_at: None,
    };
    let mut invalid_source = source.clone();
    invalid_source.id = "invalid_source".into();
    invalid_source.name = "Invalid".into();
    invalid_source.secret_ref = invalid_secret_ref.clone();
    invalid_source.models = vec!["invalid-model".into()];
    invalid_source.base_url = "not-a-url".into();
    state
        .store()
        .unwrap()
        .replace_records(vec![source, invalid_source], vec![key])
        .unwrap();
    let runtime = runtime_from_store(&state).await.unwrap();
    let address = state.gateway.start(runtime, 0).await.unwrap();
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{address}/v1/models"))
        .bearer_auth("old-secret")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let models = response.json::<serde_json::Value>().await.unwrap();
    let model_ids = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<Vec<_>>();
    assert!(model_ids.contains(&"old-model"));
    assert!(!model_ids.contains(&"invalid-model"));
    drop(client);

    state.gateway.stop().await;
    secret_store::delete(&source_secret_ref).unwrap();
    secret_store::delete(&invalid_secret_ref).unwrap();
    secret_store::delete(&key_secret_ref).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn occupied_new_port_restores_settings_and_previous_listener() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-port-rollback-{id}"));
    let source_secret_ref = format!("source:port-rollback-{id}");
    let key_secret_ref = format!("key:port-rollback-{id}");
    let state = DesktopState::open(root.clone()).unwrap();
    secret_store::save(&source_secret_ref, "upstream-secret").unwrap();
    secret_store::save(&key_secret_ref, "local-secret").unwrap();
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
    state
        .store()
        .unwrap()
        .upsert_key(LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Default".into(),
            enabled: true,
            system: true,
            secret_ref: key_secret_ref.clone(),
            created_at: "2026-07-11T00:00:00Z".into(),
            last_used_at: None,
        })
        .unwrap();
    let address = state
        .gateway
        .start(runtime_from_store(&state).await.unwrap(), 0)
        .await
        .unwrap();
    let mut old_gateway = state.store().unwrap().gateway().clone();
    old_gateway.port = address.port();
    state
        .store()
        .unwrap()
        .replace_gateway(old_gateway.clone())
        .unwrap();
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut next_gateway = old_gateway.clone();
    next_gateway.port = occupied.local_addr().unwrap().port();
    state
        .store()
        .unwrap()
        .replace_gateway(next_gateway)
        .unwrap();

    assert!(sync_gateway_or_rollback(&state, old_gateway.clone())
        .await
        .is_err());
    assert_eq!(state.store().unwrap().gateway().port, old_gateway.port);
    assert_eq!(state.gateway.address().await, Some(address));

    drop(occupied);
    state.gateway.stop().await;
    secret_store::delete(&source_secret_ref).unwrap();
    secret_store::delete(&key_secret_ref).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

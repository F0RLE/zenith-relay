use super::*;

#[tokio::test]
async fn management_token_can_rotate_without_changing_server_identity() {
    let root = TempDir::new().unwrap();
    let old_token = "synthetic-management-token-old";
    let new_token = "synthetic-management-token-new";
    let client = reqwest::Client::new();
    let first = spawn_server_with_token(root.path(), old_token).await;
    let first_health: Value = client
        .get(format!("{}/health", first.origin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = first_health["serverId"].as_str().unwrap().to_string();
    first.task.abort();
    let _ = first.task.await;
    drop(first.state);

    let restarted = spawn_server_with_token(root.path(), new_token).await;
    let restarted_health: Value = client
        .get(format!("{}/health", restarted.origin))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restarted_health["serverId"], server_id);
    assert_eq!(
        client
            .get(format!("{}/state", restarted.origin))
            .bearer_auth(old_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(format!("{}/state", restarted.origin))
            .bearer_auth(new_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn legacy_user_gateway_routes_are_removed() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    for method in ["GET", "POST", "PATCH", "DELETE"] {
        let response = client
            .request(method.parse().unwrap(), format!("{}/keys", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} /keys");
    }
    server.task.abort();
}

#[tokio::test]
async fn startup_retires_legacy_user_keys_and_restores_the_system_key() {
    let root = TempDir::new().unwrap();
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.path().to_path_buf(),
        public_base_url: url::Url::parse("http://127.0.0.1:1").unwrap(),
        management_token: "synthetic-management-token-value".to_string(),
        vault_key: [9; 32],
        account_check_url: url::Url::parse(
            zenith_relay_server::config::DEFAULT_CODEX_ACCOUNT_CHECK_URL,
        )
        .unwrap(),
    };
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    let legacy = GatewayKeyRecord {
        id: "key_legacy_user".to_string(),
        label: "Legacy user key".to_string(),
        enabled: true,
        system: false,
        secret_ref: "key:key_legacy_user".to_string(),
        created_at_ms: 0,
        last_used_at_ms: None,
    };
    let system_like = GatewayKeyRecord {
        id: "key_system".to_string(),
        label: "Legacy system key".to_string(),
        enabled: true,
        system: false,
        secret_ref: "key:key_system".to_string(),
        created_at_ms: 0,
        last_used_at_ms: None,
    };
    vault.save(&legacy.secret_ref, "legacy-secret").unwrap();
    vault
        .save(&system_like.secret_ref, "system-secret")
        .unwrap();
    store.save_key(&legacy).unwrap();
    store.save_key(&system_like).unwrap();

    let _state = AppState::new(config, store.clone(), vault.clone()).unwrap();
    let keys = store.keys().unwrap();
    assert!(keys.iter().all(|key| key.system));
    assert!(keys.iter().any(|key| key.id == "key_system"));
    assert!(vault.load(&legacy.secret_ref).unwrap().is_none());
    assert_eq!(
        vault.load(&system_like.secret_ref).unwrap().as_deref(),
        Some("system-secret")
    );
}

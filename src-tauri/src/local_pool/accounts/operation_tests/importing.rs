use super::*;

#[tokio::test]
async fn hybrid_agent_import_preserves_oauth_for_subscription_metadata() {
    const PRIVATE_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let mut parsed = parse_import(
        &serde_json::json!({
            "email": "hybrid@example.test",
            "account_id": "account-hybrid",
            "access_token": "access-hybrid",
            "refresh_token": "refresh-hybrid",
            "agent_private_key": PRIVATE_KEY,
            "agent_runtime_id": "runtime-hybrid",
            "task_id": "task-hybrid"
        })
        .to_string(),
        None,
        &[],
    )
    .unwrap();
    let (account_check_endpoint, account_check_server) =
        spawn_import_account_check_server("account-hybrid").await;
    let material = build_import_credential_material(
        parsed.items.remove(0),
        1,
        None,
        None,
        None,
        30,
        &account_check_endpoint,
    )
    .await
    .unwrap();

    assert!(material
        .authorization(1_700_000_000_000)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("AgentAssertion "));
    assert_eq!(
        material
            .subscription_authorization()
            .unwrap()
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer access-hybrid"
    );
    let stored = material.into_stored("account_local_hybrid", 1, 0).unwrap();
    assert!(stored.is_agent_identity());
    assert!(stored.has_oauth());
    assert_eq!(stored.refresh_token(), Some("refresh-hybrid"));
    account_check_server.abort();
}
#[test]
fn large_import_preview_defers_quota_network_calls() {
    assert!(should_probe_import_quota(true, QUOTA_REFRESH_BATCH_SIZE));
    assert!(!should_probe_import_quota(
        true,
        QUOTA_REFRESH_BATCH_SIZE + 1
    ));
    assert!(!should_probe_import_quota(false, 1));
}
#[test]
fn selected_import_files_are_read_and_combined_only_in_rust() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-files-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let paths = (1..=3)
        .map(|index| {
            let path = root.join(format!("account-{index}.json"));
            std::fs::write(
                &path,
                serde_json::json!({
                    "account_id": format!("provider-{index}"),
                    "access_token": format!("synthetic-access-{index}")
                })
                .to_string(),
            )
            .unwrap();
            path
        })
        .collect::<Vec<_>>();

    let documents = read_import_documents(paths).unwrap();
    let combined = combine_import_documents(&documents).unwrap();
    let parsed = parse_import(&combined, None, &[]).unwrap();

    assert_eq!(parsed.items.len(), 3);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn dropped_import_accepts_txt_tokens_and_rejects_other_extensions() {
    let txt_path = std::env::temp_dir().join(format!(
        "zenith-relay-import-{}.txt",
        Uuid::new_v4().simple()
    ));
    std::fs::write(&txt_path, "at-synthetic-token").unwrap();
    assert_eq!(
        read_import_documents(vec![txt_path.clone()]).unwrap(),
        ["at-synthetic-token"]
    );
    std::fs::remove_file(txt_path).unwrap();

    let unsupported_path = std::env::temp_dir().join(format!(
        "zenith-relay-import-{}.md",
        Uuid::new_v4().simple()
    ));
    std::fs::write(&unsupported_path, "at-synthetic-token").unwrap();
    assert!(read_import_documents(vec![unsupported_path.clone()]).is_err());
    std::fs::remove_file(unsupported_path).unwrap();
}
#[tokio::test]
async fn batch_confirm_persists_every_selected_account_and_credential() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-batch-import-{}",
        Uuid::new_v4().simple()
    ));
    let mut state = DesktopState::open(root.clone()).unwrap();
    let (account_check_endpoint, account_check_server) =
        spawn_import_account_check_server_for_accounts(&[
            "synthetic-provider-1",
            "synthetic-provider-2",
            "synthetic-provider-3",
        ])
        .await;
    state.set_account_check_url_for_test(account_check_endpoint);
    let documents = (1..=3)
        .map(|index| {
            serde_json::json!({
                "name": format!("Imported {index}"),
                "credentials": {
                    "access_token": format!("synthetic-access-{index}"),
                    "refresh_token": format!("synthetic-refresh-{index}"),
                    "chatgpt_account_id": format!("synthetic-provider-{index}"),
                    "email": format!("member-{index}@example.test"),
                    "subscription_expires_at": format!("2026-08-0{index}T00:00:00Z")
                }
            })
            .to_string()
        })
        .collect::<Vec<_>>();
    let (content, _) = normalize_import_input(StartAccountImportInput {
        content: None,
        documents,
        source_file: None,
    })
    .unwrap();
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions.start(&content, None, &[]).unwrap();
    let selected_item_ids = session
        .preview
        .rows
        .iter()
        .map(|row| row.item_id.clone())
        .collect::<Vec<_>>();

    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: session.session_id,
            selected_item_ids,
            add_to_pool: true,
            discover_models: false,
            probe_quota: false,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results.len(), 3);
    assert!(response
        .results
        .iter()
        .all(|result| result.status == ImportItemStatus::Succeeded));
    let accounts = state.store().unwrap().accounts().to_vec();
    assert_eq!(accounts.len(), 3);
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.account.id.as_str())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let mut provider_ids = HashSet::new();
    for account in &accounts {
        let credentials = credential_store.require(&account.account.id).unwrap();
        provider_ids.insert(credentials.provider_account_id().unwrap().to_string());
        credential_store.delete(&account.account.id).unwrap();
    }
    assert_eq!(provider_ids.len(), 3);
    assert!(accounts
        .iter()
        .all(|account| account.account.subscription.active_until_ms.is_some()));
    assert!(accounts.iter().all(|account| account.account.in_pool));
    assert!(state
        .store()
        .unwrap()
        .accounts()
        .iter()
        .any(|record| record.account.is_automatic_quota_monitoring_eligible()));

    account_check_server.abort();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn access_only_reimport_preserves_existing_refresh_token() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-refresh-preserve-{}",
        Uuid::new_v4().simple()
    ));
    let mut state = DesktopState::open(root.clone()).unwrap();
    let (account_check_endpoint, account_check_server) =
        spawn_import_account_check_server("provider-preserve").await;
    state.set_account_check_url_for_test(account_check_endpoint);
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let first = sessions
            .start(
                r#"{"auth_mode":"chatgpt","account_id":"provider-preserve","access_token":"access-original","refresh_token":"refresh-original"}"#,
                None,
                &[],
            )
            .unwrap();
    let first_item_id = first.preview.rows[0].item_id.clone();
    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: first.session_id,
            selected_item_ids: vec![first_item_id],
            add_to_pool: false,
            discover_models: false,
            probe_quota: false,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();
    let account_id = response.results[0]
        .account
        .as_ref()
        .unwrap()
        .account
        .id
        .clone();

    let second = sessions
        .start(
            r#"{"account_id":"provider-preserve","access_token":"access-replacement"}"#,
            None,
            &[],
        )
        .unwrap();
    let second_item_id = second.preview.rows[0].item_id.clone();
    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: second.session_id,
            selected_item_ids: vec![second_item_id],
            add_to_pool: false,
            discover_models: false,
            probe_quota: false,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results[0].status, ImportItemStatus::Succeeded);
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let credentials = credential_store.require(&account_id).unwrap();
    assert_eq!(credentials.access_token(), "access-replacement");
    assert_eq!(credentials.refresh_token(), Some("refresh-original"));
    assert_eq!(
        state
            .store()
            .unwrap()
            .account(&account_id)
            .unwrap()
            .account
            .auth_mode,
        AccountAuthMode::OAuth
    );
    account_check_server.abort();
    credential_store.delete(&account_id).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn import_outside_pool_is_scheduled_for_quota_monitoring() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-retry-{}",
        Uuid::new_v4().simple()
    ));
    let mut state = DesktopState::open(root.clone()).unwrap();
    let (account_check_endpoint, account_check_server) =
        spawn_import_account_check_server("provider-retry").await;
    state.set_account_check_url_for_test(account_check_endpoint);
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .start(
            r#"{"account_id":"provider-retry","access_token":"access-retry"}"#,
            None,
            &[],
        )
        .unwrap();
    let item_id = session.preview.rows[0].item_id.clone();
    let session_id = session.session_id.clone();

    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: session_id.clone(),
            selected_item_ids: vec![item_id],
            add_to_pool: false,
            discover_models: false,
            probe_quota: false,
            models: Vec::new(),
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results[0].status, ImportItemStatus::Succeeded);
    let account = response.results[0].account.as_ref().unwrap();
    assert!(account.models.is_empty());
    assert!(state
        .store()
        .unwrap()
        .accounts()
        .iter()
        .any(|record| record.account.is_automatic_quota_monitoring_eligible()));
    assert!(sessions.resume(&session_id, &[]).is_err());
    account_check_server.abort();
    CredentialStore::from_backend(NativeSecretBackend)
        .delete(&account.account.id)
        .unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn cockpit_api_keys_do_not_require_oauth_quota_preview() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-cockpit-source-import-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let content = r#"[
            {"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-key-one","api_base_url":"https://one.example.test/v1","api_provider_name":"One API"},
            {"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-key-two","api_base_url":"https://two.example.test/v1","api_provider_name":"Two API"}
        ]"#;
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions.start(content, None, &[]).unwrap();
    let selected_item_ids = session
        .preview
        .rows
        .iter()
        .map(|row| row.item_id.clone())
        .collect();

    let response = confirm_local_account_import_inner(
        ConfirmAccountImportInput {
            session_id: session.session_id,
            selected_item_ids,
            add_to_pool: true,
            discover_models: false,
            probe_quota: true,
            models: vec!["gpt-test".into()],
        },
        &state,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.results.len(), 2);
    assert!(response
        .results
        .iter()
        .all(|result| result.status == ImportItemStatus::Succeeded));
    let sources = state.store().unwrap().sources().to_vec();
    assert_eq!(sources.len(), 2);
    assert!(sources.iter().all(|source| source.in_pool));
    assert_eq!(
        sources
            .iter()
            .map(|source| source.name.as_str())
            .collect::<HashSet<_>>(),
        HashSet::from(["One API", "Two API"])
    );
    for source in sources {
        secret_store::delete(&source.secret_ref).unwrap();
    }

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn api_key_auth_json_builds_a_safe_default_responses_source() {
    let mut parsed = parse_import(
        r#"{"auth_mode":"api_key","OPENAI_API_KEY":"sk-private"}"#,
        None,
        &[],
    )
    .unwrap();
    let item = parsed.items.remove(0);
    let base_url = imported_source_base_url(&item).unwrap();
    let wire_api = imported_source_wire_api(&item, None).unwrap();
    let runtime = ProviderSource {
        id: "source_test".into(),
        name: item.label.clone(),
        base_url,
        api_key: item.secrets().api_key().unwrap().to_string(),
        wire_api,
        models: vec!["gpt-test".into()],
    };
    runtime.validate().unwrap();
    let source = imported_source_record(
        &item,
        runtime,
        "source:source_test".into(),
        None,
        Default::default(),
        Default::default(),
        Default::default(),
        None,
    );
    let serialized = serde_json::to_string(&ImportItemResult::source_success(
        item.item_id,
        source.clone(),
    ))
    .unwrap();

    assert_eq!(source.base_url, DEFAULT_OPENAI_SOURCE_URL);
    assert_eq!(source.wire_api, WireApi::Responses);
    assert_eq!(source.models, ["gpt-test"]);
    assert!(!serialized.contains("sk-private"));
    assert!(serialized.contains("source"));
}
#[test]
fn source_duplicate_identity_updates_the_existing_local_record() {
    let mut parsed = parse_import(
        r#"{"api_key":"sk-private","base_url":"https://api.example.test/v1/"}"#,
        None,
        &[],
    )
    .unwrap();
    let item = parsed.items.remove(0);
    let existing = ProviderSourceRecord {
        id: "source_existing".into(),
        name: "Custom name".into(),
        enabled: false,
        in_pool: true,
        draining: true,
        base_url: "https://api.example.test/v1".into(),
        secret_ref: "source:source_existing".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::ChatCompletions,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["old-model".into()],
        allowed_models: vec!["gpt-*".into()],
        excluded_models: vec!["gpt-old".into()],
        priority: 7,
        weight: 3,
        recovery_delay_seconds: 60,
        model_price_overrides: Default::default(),
        detected_model_prices: Default::default(),
        last_used_at: Some("2026-07-10T00:00:00Z".into()),
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    };
    assert_eq!(
        source_identity_key(&existing.base_url, "sk-private").unwrap(),
        source_identity_key(item.base_url.as_deref().unwrap(), "sk-private").unwrap()
    );
    let wire_api = imported_source_wire_api(&item, Some(&existing)).unwrap();
    let runtime = ProviderSource {
        id: existing.id.clone(),
        name: existing.name.clone(),
        base_url: imported_source_base_url(&item).unwrap(),
        api_key: "sk-private".into(),
        wire_api,
        models: vec!["new-model".into()],
    };
    let discovered_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::ChatCompletions,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["new-model".into()],
    }];
    let updated = imported_source_record(
        &item,
        runtime,
        existing.secret_ref.clone(),
        Some(&existing),
        existing.protocol_config.clone(),
        discovered_bindings.clone(),
        existing.detected_model_prices.clone(),
        None,
    );

    assert_eq!(updated.id, existing.id);
    assert_eq!(updated.name, existing.name);
    assert_eq!(updated.wire_api, WireApi::ChatCompletions);
    assert_eq!(updated.protocol_bindings, discovered_bindings);
    assert_eq!(updated.models, ["new-model"]);
    assert_eq!(updated.allowed_models, existing.allowed_models);
    assert_eq!(updated.excluded_models, existing.excluded_models);
    assert_eq!(updated.priority, 7);
    assert_eq!(updated.weight, 3);
    assert_eq!(updated.recovery_delay_seconds, 60);
    assert!(!updated.enabled);
    assert!(updated.draining);
}
#[tokio::test]
async fn source_import_rebuilds_legacy_protocol_routes_automatically() {
    let id = Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-source-import-invalid-{id}"));
    let secret_ref = format!("source:import-invalid-{id}");
    let state = DesktopState::open(root.clone()).unwrap();
    secret_store::save(&secret_ref, "sk-import-test").unwrap();
    let source = ProviderSourceRecord {
        id: "source_existing_invalid".into(),
        name: "Existing invalid source".into(),
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: "https://api.example.test/v1".into(),
        secret_ref: secret_ref.clone(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: vec![SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-test".into()],
        }],
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

    let mut parsed = parse_import(
        r#"{"api_key":"sk-import-test","base_url":"https://api.example.test/v1"}"#,
        None,
        &[],
    )
    .unwrap();
    let item = parsed.items.remove(0);
    let result = import_source_item(&state, item, false, false, &["gpt-test".to_string()]).await;
    let updated = result.unwrap();
    assert_eq!(updated.id, source.id);
    let routes = updated.effective_protocol_bindings().unwrap();
    assert_eq!(routes.len(), WireApi::ALL.len());
    assert!(routes.iter().all(|route| route.model_ids == ["gpt-test"]));
    assert_eq!(state.store().unwrap().source(&source.id), Some(&source));

    secret_store::delete(&secret_ref).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn missing_import_secret_requires_recovery() {
    let error = import_session_error(ImportSessionError {
        code: ImportSessionErrorCode::SecretMissing,
        message: "import session secret is missing".into(),
        session_id: None,
        import_code: None,
    });
    assert!(serde_json::to_string(&error)
        .unwrap()
        .contains("recovery_required"));
}

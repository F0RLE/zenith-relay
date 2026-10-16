use super::*;

#[test]
fn moved_accounts_remain_stored_but_leave_local_routing() {
    let mut moved = account_record("account-moved");
    moved.account.in_pool = true;
    let untouched = account_record("account-untouched");
    let mut accounts = vec![moved, untouched.clone()];

    let location = RemoteAccountLocation {
        server_id: "server-one".into(),
        remote_account_id: "account-remote".into(),
    };
    mark_local_accounts_moved(
        &mut accounts,
        &HashMap::from([("account-moved".to_string(), location.clone())]),
    )
    .unwrap();

    assert!(!accounts[0].account.enabled);
    assert!(!accounts[0].account.in_pool);
    assert_eq!(accounts[0].remote_location, Some(location));
    assert_eq!(accounts[1], untouched);
    assert!(mark_local_accounts_moved(
        &mut accounts,
        &HashMap::from([(
            "account-missing".to_string(),
            RemoteAccountLocation {
                server_id: "server-one".into(),
                remote_account_id: "account-missing".into(),
            },
        )]),
    )
    .is_err());
}
#[test]
fn revealable_identity_prefers_email_and_falls_back_to_provider_account() {
    let with_email = StoredCodexCredentials::new(
        "account_email",
        "access-private".into(),
        None,
        None,
        None,
        1,
        0,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        Some("provider-user".into()),
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        revealable_account_identity(&with_email),
        Some("private@example.test")
    );

    let without_email = StoredCodexCredentials::new(
        "account_provider",
        "access-private".into(),
        None,
        None,
        None,
        1,
        0,
        None,
        Some("provider-account".into()),
        Some("provider-user".into()),
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        revealable_account_identity(&without_email),
        Some("provider-account")
    );
}
#[test]
fn export_restores_generated_identity_but_preserves_custom_labels() {
    let credentials = StoredCodexCredentials::new(
        "account_export",
        "access-private".into(),
        None,
        None,
        None,
        1,
        0,
        Some("private@example.test".into()),
        Some("provider-account".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let masked = credentials.snapshot().identity.unwrap();

    assert_eq!(
        export_account_label(&masked, &credentials),
        "private@example.test"
    );
    assert_eq!(export_account_label("Work Plus", &credentials), "Work Plus");
}
#[test]
fn selected_ids_are_validated_and_deduplicated_in_order() {
    let selected = normalize_selected_item_ids(vec![
        "import_0123456789abcdef".into(),
        " import_0123456789abcdef ".into(),
        "import_fedcba9876543210".into(),
    ])
    .unwrap();
    assert_eq!(
        selected,
        [
            "import_0123456789abcdef".to_string(),
            "import_fedcba9876543210".to_string()
        ]
    );
    assert!(normalize_selected_item_ids(vec!["../secret".into()]).is_err());
}
#[test]
fn current_codex_profile_reads_only_its_auth_document() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-current-codex-import-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let content = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-current-key"}"#;
    std::fs::write(root.join("auth.json"), content).unwrap();

    let documents = current_codex_import_documents(&root, &[]).unwrap();

    assert_eq!(documents, [content]);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn current_codex_profile_rejects_an_active_local_gateway_projection() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-managed-codex-import-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("auth.json"),
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-local-key"}"#,
    )
    .unwrap();
    let binding = codex::ProfileBinding {
        profile_dir: root.to_string_lossy().into_owned(),
        credential_kind: codex::ProfileCredentialKind::LocalGateway,
        credential_id: "local_gateway".into(),
        bound_oauth_account_id: None,
        active: true,
    };

    let error = current_codex_import_documents(&root, &[binding]).unwrap_err();

    assert!(matches!(error.code, ErrorCode::Conflict));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn current_chatgpt_profile_visibility_requires_refreshable_oauth_identity() {
    let oauth = parse_import(
            r#"{"auth_mode":"chatgpt","account_id":"provider-current","access_token":"access-current","refresh_token":"refresh-current"}"#,
            Some("auth.json"),
            &[],
        )
        .unwrap();
    let api_key = parse_import(
        r#"{"auth_mode":"apikey","account_id":"provider-key","OPENAI_API_KEY":"key-current"}"#,
        Some("auth.json"),
        &[],
    )
    .unwrap();

    assert!(is_usable_current_chatgpt_profile(&oauth, current_time_ms()));
    assert!(!is_usable_current_chatgpt_profile(
        &api_key,
        current_time_ms()
    ));
}
#[test]
fn expired_local_access_metadata_does_not_hide_a_refreshable_chatgpt_profile() {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "exp": 1,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "provider-expired"
            }
        })
        .to_string(),
    );
    let access_token = format!("header.{payload}.signature");
    let parsed = parse_import(
        &serde_json::json!({
            "auth_mode": "chatgpt",
            "account_id": "provider-expired",
            "access_token": access_token,
            "refresh_token": "refresh-expired-access-metadata"
        })
        .to_string(),
        Some("auth.json"),
        &[],
    )
    .unwrap();

    assert!(is_usable_current_chatgpt_profile(&parsed, u64::MAX));
}
#[test]
fn account_patch_normalizes_metadata_and_rejects_zero_weight() {
    let credentials = StoredCodexCredentials::new(
        "account_local",
        "access-private".into(),
        Some("refresh-private".into()),
        None,
        None,
        1,
        0,
        None,
        Some("provider-private".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let mut account = records::new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        1,
    )
    .unwrap();
    apply_account_patch(
        &mut account,
        UpdateAccountInput {
            account_id: "account_local".into(),
            label: Some("  Personal  ".into()),
            priority: Some(7),
            weight: Some(2),
            allowed_models: Some(vec![" gpt-test ".into(), "gpt-test".into()]),
            excluded_models: Some(vec![" gpt-old ".into()]),
            in_pool: Some(true),
            draining: Some(true),
            purchase_cost_micro_usd: Some(12_500_000),
        },
    )
    .unwrap();
    assert_eq!(account.account.label, "Personal");
    assert_eq!(account.priority, 7);
    assert_eq!(account.weight, 2);
    assert_eq!(account.allowed_models, ["gpt-test"]);
    assert_eq!(account.excluded_models, ["gpt-old"]);
    assert_eq!(account.purchase_cost_micro_usd, Some(12_500_000));
    assert!(account.account.draining);
    assert!(apply_account_patch(
        &mut account,
        UpdateAccountInput {
            account_id: "account_local".into(),
            label: None,
            priority: None,
            weight: Some(0),
            allowed_models: None,
            excluded_models: None,
            in_pool: None,
            draining: None,
            purchase_cost_micro_usd: None,
        },
    )
    .is_err());
}
#[test]
fn deleting_account_prunes_explicit_selectors_without_rewriting_wake_state() {
    let mut automations = AutomationRecords::default();
    let original_state = automations.state.clone();
    automations.tasks = vec![
        wake_task("only-deleted", &["account_1"]),
        wake_task("shared", &["account_1", "account_2"]),
    ];

    let pruned = prune_account_task_selectors(automations, "account_1");

    assert_eq!(pruned.tasks.len(), 1);
    assert_eq!(pruned.tasks[0].id, "shared");
    assert_eq!(
        pruned.tasks[0].account_selector,
        AccountSelector::AccountIds(BTreeSet::from(["account_2".to_string()]))
    );
    assert_eq!(pruned.state, original_state);
}
#[test]
fn bulk_delete_preflights_every_account_before_mutation() {
    let accounts = vec![account_record("account_1"), account_record("account_2")];
    assert!(ensure_accounts_exist(&accounts, &["account_1".into(), "account_2".into()],).is_ok());
    let error =
        ensure_accounts_exist(&accounts, &["account_1".into(), "missing".into()]).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(error.message.contains("missing"));
}
#[test]
fn failed_delete_restores_credentials_quota_and_profile_binding() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-delete-rollback-{}",
        Uuid::new_v4().simple()
    ));
    let profile = root.join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("config.toml"), "model_provider = \"custom\"\n").unwrap();
    let state = DesktopState::open(root.clone()).unwrap();
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let stored = StoredCodexCredentials::new(
        &account_id,
        "access-private".into(),
        Some("refresh-private".into()),
        None,
        Some(60_000),
        1,
        1,
        None,
        Some("provider-private".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    credentials.save(&stored).unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(account_record(&account_id))
        .unwrap();
    let before_refresh = state
        .store()
        .unwrap()
        .account_refresh_scope(&account_id)
        .unwrap()
        .1;
    codex::attach_account(
        &profile,
        &state.profile_backup_root(),
        &account_id,
        &stored.to_token_set().unwrap(),
        "provider-private",
    )
    .unwrap();

    let previous_wake = state.wake_snapshot().unwrap();
    let old_automations = state.store().unwrap().automations().clone();
    let bindings = codex::account_bindings(&state.profile_backup_root()).unwrap();
    let restored = restore_bound_account_profiles(&state, &bindings, Some(&stored)).unwrap();
    credentials.delete(&account_id).unwrap();
    state
        .store()
        .unwrap()
        .invalidate_account_refresh(&[&account_id])
        .unwrap();
    state.remove_account_refresh(&account_id);

    rollback_deleted_account_side_effects(
        &state,
        &credentials,
        Some(&stored),
        previous_wake,
        old_automations,
        &restored,
        None,
        &LocalPoolError::new(ErrorCode::Io, "injected delete failure"),
    )
    .unwrap();

    assert!(credentials.require(&account_id).is_ok());
    assert!(state
        .store()
        .unwrap()
        .ensure_account_refresh_current(&before_refresh)
        .is_err());
    assert!(state
        .sync_account_quota_refresh(&account_id, current_time_ms())
        .unwrap());
    assert_eq!(
        codex::account_bindings(&state.profile_backup_root())
            .unwrap()
            .len(),
        1
    );
    assert!(std::fs::read_to_string(profile.join("auth.json"))
        .unwrap()
        .contains("access-private"));

    codex::restore_account_profile(&profile, &state.profile_backup_root()).unwrap();
    credentials.delete(&account_id).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn pre_authority_refresh_failure_restores_credentials_and_account_state() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-refresh-rollback-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let previous = StoredCodexCredentials::new(
        &account_id,
        "access-before".into(),
        Some("refresh-before".into()),
        None,
        Some(60_000),
        1,
        1,
        None,
        Some("provider-private".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let refreshed = StoredCodexCredentials::new(
        &account_id,
        "access-after".into(),
        Some("refresh-after".into()),
        None,
        Some(120_000),
        2,
        2,
        None,
        Some("provider-private".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    credentials.save(&previous).unwrap();
    let old_record = records::new_account_record(
        &previous,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        1,
    )
    .unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(old_record.clone())
        .unwrap();
    let old_accounts = {
        let store = state.store().unwrap();
        store.accounts().to_vec()
    };
    let previous_tokens = previous.to_token_set().unwrap();
    state
        .token_authority()
        .register(
            &account_id,
            previous_tokens.clone(),
            old_record.account.auth_state,
        )
        .await
        .unwrap();

    // Simulate a failure after the reversible credential and account writes,
    // before TokenAuthority is updated.
    credentials.save(&refreshed).unwrap();
    let mut changed_record = old_record.clone();
    changed_record.account.token_generation = refreshed.generation();
    changed_record.account.token_updated_at_ms = Some(2);
    state
        .store()
        .unwrap()
        .upsert_account(changed_record)
        .unwrap();

    let error = rollback_force_refreshed_before_authority(
        &state,
        &credentials,
        &account_id,
        &previous,
        &previous_tokens,
        &refreshed.to_token_set().unwrap(),
        &old_accounts,
        "provider-private",
        false,
        LocalPoolError::new(ErrorCode::Io, "injected refresh failure"),
    )
    .await;

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(
        credentials.require(&account_id).unwrap().access_token(),
        previous.access_token()
    );
    assert_eq!(
        state
            .token_authority()
            .tokens(&account_id)
            .await
            .unwrap()
            .access_token(),
        previous.access_token()
    );
    assert_eq!(
        state.store().unwrap().account(&account_id),
        Some(&old_record)
    );

    let mut newer_record = old_record.clone();
    newer_record.account.token_generation = refreshed.generation().saturating_add(1);
    newer_record.account.token_updated_at_ms = Some(3);
    state
        .store()
        .unwrap()
        .upsert_account(newer_record.clone())
        .unwrap();
    assert!(!restore_force_refreshed_account_record(
        &state,
        &account_id,
        &refreshed.to_token_set().unwrap(),
        &old_accounts,
    )
    .unwrap());
    assert_eq!(
        state.store().unwrap().account(&account_id),
        Some(&newer_record)
    );

    credentials.delete(&account_id).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn refreshed_authority_reconciliation_keeps_a_real_client_login_warning() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-refresh-watchdog-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let mut record = account_record(&account_id);
    record.client_auth_status = Some("login_required".into());
    record.last_client_login_redirect_at_ms = Some(123);
    state.store().unwrap().upsert_account(record).unwrap();
    let attempted = TokenSet::new(
        "access-before",
        Some("refresh-before".into()),
        None,
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    let authoritative = TokenSet::new(
        "access-after",
        Some("refresh-after".into()),
        None,
        Some(120_000),
        2,
        2,
    )
    .unwrap();

    reconcile_force_refreshed_account_record(
        &state,
        &account_id,
        &attempted,
        &authoritative,
        AccountAuthState::Active,
    )
    .unwrap();
    let persisted = state
        .store()
        .unwrap()
        .account(&account_id)
        .cloned()
        .unwrap();
    assert_eq!(
        persisted.client_auth_status.as_deref(),
        Some("login_required")
    );
    assert_eq!(persisted.last_client_login_redirect_at_ms, Some(123));

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn prepared_credentials_debug_output_is_redacted() {
    let prepared = PreparedAccountCredentials {
        oauth_client_kind: Default::default(),
        tokens: TokenSet::new(
            "access-private",
            Some("refresh-private".into()),
            None,
            Some(60_000),
            1,
            0,
        )
        .unwrap(),
        provider_account_id: "provider-private".into(),
        proxy: None,
    };
    assert_eq!(prepared.tokens().access_token(), "access-private");
    let debug = format!("{prepared:?}");
    assert!(!debug.contains("access-private"));
    assert!(!debug.contains("refresh-private"));
    assert!(!debug.contains("provider-private"));
}
#[test]
fn session_and_item_responses_never_serialize_secret_material() {
    let parsed = parse_import(
            r#"{"account_id":"provider-private","access_token":"access-private","refresh_token":"refresh-private"}"#,
            None,
            &[],
        )
        .unwrap();
    let response = ImportSessionResponse {
        session_id: "session-safe".into(),
        created_at_ms: 1,
        prepared: false,
        preview: parsed.preview,
    };
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(!serialized.contains("access-private"));
    assert!(!serialized.contains("refresh-private"));
    assert!(!serialized.contains("provider-private"));
    assert!(!serialized.contains("items"));

    let failed = ImportItemResult::failure(
        "import_0123456789abcdef".into(),
        ImportItemError::new("use_source_import", "use the source import flow"),
    );
    let serialized = serde_json::to_string(&failed).unwrap();
    assert!(!serialized.contains("access-private"));
}

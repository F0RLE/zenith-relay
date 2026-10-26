use super::*;

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
fn account_patch_normalizes_metadata_and_rejects_zero_weight() {
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let credentials = StoredCodexCredentials::new(
        &account_id,
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
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    credential_store.save(&credentials).unwrap();
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
            account_id: account_id.clone(),
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
    assert!(account.account.in_pool);
    assert!(account.image_pool_enabled);
    assert!(account.account.draining);
    assert!(
        apply_account_patch(
            &mut account,
            UpdateAccountInput {
                account_id: account_id.clone(),
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
        .is_err()
    );
    credential_store.delete(&account_id).unwrap();
}
#[test]
fn account_patch_cannot_join_pool_without_stored_credentials() {
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let mut account = account_record(&account_id);
    let before = account.clone();
    let error = apply_account_patch(
        &mut account,
        UpdateAccountInput {
            account_id,
            label: None,
            priority: None,
            weight: None,
            allowed_models: None,
            excluded_models: None,
            in_pool: Some(true),
            draining: None,
            purchase_cost_micro_usd: None,
        },
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidState);
    assert_eq!(error.message, "stored ChatGPT credentials are missing");
    assert_eq!(account, before);
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
        &crate::local_pool::accounts::mutations::delete::PreparedAccountDelete {
            provider_revoked: false,
            old_credential: Some(stored.clone()),
            previous_wake,
            old_automations,
            restored_bindings: restored,
            previous_proxy_pool: None,
        },
        &LocalPoolError::new(ErrorCode::Io, "injected delete failure"),
    )
    .unwrap();

    assert!(credentials.require(&account_id).is_ok());
    assert!(
        state
            .store()
            .unwrap()
            .ensure_account_refresh_current(&before_refresh)
            .is_err()
    );
    assert!(
        state
            .sync_account_quota_refresh(&account_id, current_time_ms())
            .unwrap()
    );
    assert_eq!(
        codex::account_bindings(&state.profile_backup_root())
            .unwrap()
            .len(),
        1
    );
    assert!(
        std::fs::read_to_string(profile.join("auth.json"))
            .unwrap()
            .contains("access-private")
    );

    codex::restore_account_profile(&profile, &state.profile_backup_root()).unwrap();
    credentials.delete(&account_id).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn registered_account_projection_preserves_newer_tokens_and_reauth_state() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-authority-projection-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let account_id = format!("account_{}", Uuid::new_v4().simple());
    let expected = TokenSet::new(
        "access-before",
        Some("refresh-before".into()),
        None,
        Some(60_000),
        100,
        1,
    )
    .unwrap();
    let authoritative = TokenSet::new(
        "access-after",
        Some("refresh-after".into()),
        None,
        Some(120_000),
        200,
        2,
    )
    .unwrap();
    for (generation, updated_at_ms, auth_state) in [
        (2, 200, AccountAuthState::Active),
        (1, 101, AccountAuthState::Active),
        (
            1,
            100,
            AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
        ),
    ] {
        let mut record = account_record(&account_id);
        record.account.token_generation = generation;
        record.account.token_updated_at_ms = Some(updated_at_ms);
        record.account.auth_state = auth_state;
        record.account.last_error_code = Some("newer_state".into());
        state
            .store()
            .unwrap()
            .upsert_account(record.clone())
            .unwrap();
        reconcile_registered_account_record(
            &state,
            &account_id,
            &expected,
            &authoritative,
            AccountAuthState::Active,
        )
        .unwrap();
        assert_eq!(state.store().unwrap().account(&account_id), Some(&record));
    }
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

    reconcile_registered_account_record(
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

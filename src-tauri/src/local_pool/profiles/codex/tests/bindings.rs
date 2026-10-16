use super::*;

#[test]
fn profile_bindings_reports_orphaned_managed_provider_without_blocking_inventory() {
    let (root, home, backups) = profile_dirs("missing-reset-backup");
    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"zenith_relay_local\"\n\n[model_providers.zenith_relay_local]\nname = \"Zenith Relay\"\n",
        )
        .unwrap();

    let bindings = profile_bindings(&home, &backups).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(
        bindings[0].credential_kind,
        ProfileCredentialKind::LocalGateway
    );
    assert!(!bindings[0].active);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn oauth_account_attach_reuses_one_profile_binding_and_restores_previous_login() {
    let (root, home, backups) = profile_dirs("oauth-account");
    let previous_config = r#"model_provider = "codex_local_access"
model = "relay-model"
review_model = "relay-review-model"
model_catalog_json = "relay-catalog.json"
chatgpt_base_url = "https://relay.example.com/v1"
openai_base_url = "https://stale.example.com/v1"
model_reasoning_effort = "ultra"

[model_providers.zenith_relay_local]
name = "Zenith Relay Local"
[model_providers.codex_local_access]
name = "Codex API Service"
[model_providers.zenith]
name = "Zenith"
[model_providers.custom]
name = "Custom"
base_url = "https://custom.example.com/v1"

[profiles.work]
model = "profile-relay-model"
model_provider = "codex_local_access"
model_catalog_json = "profile-relay-catalog.json"
openai_base_url = "https://profile-relay.example.com/v1"
[profiles.native]
model_provider = "openai"
model = "gpt-native"
"#;
    fs::write(home.join(CONFIG_FILE), previous_config).unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"previous\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let first = TokenSet::new(
        "access-secret",
        Some("refresh-secret".into()),
        Some("id-secret".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    let binding = attach_account_with(
        &home,
        &backups,
        "account-local",
        &first,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    assert_eq!(binding.credential_id, "account-local");
    let stored_bindings = account_bindings(&backups).unwrap();
    assert_eq!(stored_bindings.len(), 1);
    assert_eq!(stored_bindings[0].credential_id, binding.credential_id);
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);
    let account_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(!account_config.starts_with("model_provider ="));
    assert!(!account_config.contains("model ="));
    assert!(!account_config.contains("review_model"));
    assert!(!account_config.contains("model_catalog_json"));
    assert!(!account_config.contains("chatgpt_base_url"));
    assert!(!account_config.contains("openai_base_url"));
    assert!(!account_config.contains("[model_providers.zenith_relay_local]"));
    assert!(!account_config.contains("[model_providers.codex_local_access]"));
    assert!(!account_config.contains("[model_providers.zenith]"));
    assert!(!account_config.contains("profile-relay-model"));
    assert!(!account_config.contains("profile-relay-catalog.json"));
    assert!(account_config.contains("[profiles.native]"));
    assert!(account_config.contains("[model_providers.custom]"));
    let account_auth: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
    assert_eq!(account_auth["OPENAI_API_KEY"], serde_json::Value::Null);
    assert_eq!(account_auth["tokens"]["refresh_token"], "refresh-secret");
    assert_eq!(account_auth["auth_mode"], "chatgpt");

    let canonical_home = canonical_profile_dir(&home).unwrap();
    let backup_path = account_backup_path(&backups, &canonical_home);
    let backup = fs::read_to_string(&backup_path).unwrap();
    for secret in [
        "access-secret",
        "refresh-secret",
        "id-secret",
        "provider-private-id",
    ] {
        assert!(!backup.contains(secret));
    }

    attach_account_with(
        &home,
        &backups,
        "account-local",
        &first,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    assert_eq!(account_bindings(&backups).unwrap().len(), 1);

    let refreshed = TokenSet::new(
        "access-refreshed",
        Some("refresh-new".into()),
        Some("id-new".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    assert_eq!(
        sync_account_bindings(&backups, "account-local", &refreshed, "provider-private-id",)
            .unwrap(),
        1
    );
    assert_eq!(
        sync_account_bindings(&backups, "account-local", &refreshed, "provider-private-id",)
            .unwrap(),
        0
    );
    assert_eq!(account_bindings(&backups).unwrap().len(), 1);
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("access-refreshed"));

    let restored = restore_account_with(&home, &backups, &secrets)
        .unwrap()
        .unwrap();
    assert_eq!(restored, binding);
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        previous_config
    );
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("previous"));
    assert!(account_bindings(&backups).unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_attach_deactivates_external_provider_but_keeps_its_definition() {
    let (root, home, backups) = profile_dirs("oauth-account-external-provider");
    let previous_config = r#"model_provider = "external_provider"
model = "external-model"
model_catalog_json = "external-catalog.json"

[model_providers.external_provider]
name = "External Provider"
base_url = "https://provider.example.com/v1"

[model_providers.custom]
name = "Custom"
base_url = "https://custom.example.com/v1"
"#;
    fs::write(home.join(CONFIG_FILE), previous_config).unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "account-access",
        Some("account-refresh".into()),
        None,
        None,
        1,
        1,
    )
    .unwrap();

    attach_account_with(
        &home,
        &backups,
        "account-external-provider",
        &tokens,
        "provider-external",
        &secrets,
    )
    .unwrap();

    let attached = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(!attached.contains("model_provider = \"external_provider\""));
    assert!(attached.contains("[model_providers.external_provider]"));
    assert!(attached.contains("[model_providers.custom]"));

    restore_account_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        previous_config
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_profile_rotation_is_adopted_only_for_the_same_account() {
    let (root, home, backups) = profile_dirs("managed-token-adoption");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    let secrets = MemorySecrets::default();
    let original = TokenSet::new(
        "access-original",
        Some("refresh-original".into()),
        Some("id-original".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    attach_account_with(
        &home,
        &backups,
        "account-local",
        &original,
        "provider-account",
        &secrets,
    )
    .unwrap();

    let rotated = TokenSet::new(
        "access-rotated",
        Some("refresh-rotated".into()),
        Some("id-rotated".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        account_auth_content(&rotated, "provider-account").unwrap(),
    )
    .unwrap();
    let update = managed_account_token_update(
        &home,
        &backups,
        "account-local",
        &original,
        "provider-account",
    )
    .unwrap()
    .unwrap();
    assert_eq!(update.access_token, "access-rotated");
    assert_eq!(update.refresh_token, "refresh-rotated");
    assert_eq!(update.id_token.as_deref(), Some("id-rotated"));
    let debug = format!("{update:?}");
    assert!(!debug.contains("rotated"));

    assert_eq!(
        sync_account_bindings(&backups, "account-local", &rotated, "provider-account").unwrap(),
        1
    );
    assert!(managed_account_token_update(
        &home,
        &backups,
        "account-local",
        &rotated,
        "provider-account",
    )
    .unwrap()
    .is_none());

    let other = TokenSet::new(
        "other-access",
        Some("other-refresh".into()),
        Some("other-id".into()),
        Some(180_000),
        3,
        3,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        account_auth_content(&other, "provider-other").unwrap(),
    )
    .unwrap();
    assert!(managed_account_token_update(
        &home,
        &backups,
        "account-local",
        &rotated,
        "provider-account",
    )
    .unwrap()
    .is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn managed_profile_refresh_only_rotation_updates_other_bound_profiles() {
    let (root, home, backups) = profile_dirs("managed-refresh-only-rotation");
    let peer = root.join("peer-profile");
    fs::create_dir_all(&peer).unwrap();
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(peer.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    let secrets = MemorySecrets::default();
    let original = TokenSet::new(
        "access-stable",
        Some("refresh-original".into()),
        Some("id-original".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    for profile in [&home, &peer] {
        attach_account_with(
            profile,
            &backups,
            "account-local",
            &original,
            "provider-account",
            &secrets,
        )
        .unwrap();
    }

    let rotated = TokenSet::new(
        "access-stable",
        Some("refresh-rotated".into()),
        Some("id-rotated".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        account_auth_content(&rotated, "provider-account").unwrap(),
    )
    .unwrap();

    let update = managed_account_token_update(
        &home,
        &backups,
        "account-local",
        &original,
        "provider-account",
    )
    .unwrap()
    .expect("refresh-only rotation must be adopted");
    assert_eq!(update.access_token, "access-stable");
    assert_eq!(update.refresh_token, "refresh-rotated");
    assert_eq!(update.id_token.as_deref(), Some("id-rotated"));

    assert_eq!(
        sync_account_bindings(&backups, "account-local", &rotated, "provider-account").unwrap(),
        1
    );
    let peer_auth = fs::read_to_string(peer.join(AUTH_FILE)).unwrap();
    assert!(peer_auth.contains("refresh-rotated"));
    assert!(peer_auth.contains("id-rotated"));
    assert!(managed_account_token_update(
        &home,
        &backups,
        "account-local",
        &rotated,
        "provider-account",
    )
    .unwrap()
    .is_none());

    fs::remove_dir_all(root).unwrap();
}
#[test]
fn switching_account_and_local_gateway_preserves_the_original_profile() {
    let (root, home, backups) = profile_dirs("credential-kind-switch");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "managed-account",
        Some("refresh".into()),
        Some("id-token".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    let local = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();
    assert_eq!(local.credential_kind, ProfileCredentialKind::LocalGateway);
    assert_eq!(profile_bindings(&home, &backups).unwrap(), vec![local]);
    assert_eq!(profile_backup_count(&backups), 1);

    let account = switch_to_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-account",
        &secrets,
    )
    .unwrap();
    assert_eq!(account.credential_kind, ProfileCredentialKind::OAuthAccount);
    assert_eq!(profile_bindings(&home, &backups).unwrap(), vec![account]);
    assert!(!backup_path(&backups).exists());
    assert_eq!(profile_backup_count(&backups), 1);

    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();
    assert_eq!(profile_backup_count(&backups), 1);
    restore_with(&home, &backups, &secrets).unwrap();

    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("model_provider = \"custom\""));
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("original"));
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn profile_binding_detects_an_external_provider_takeover() {
    let (root, home, backups) = profile_dirs("external-provider-active-state");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"apikey\"}").unwrap();
    let secrets = MemorySecrets::default();
    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);

    let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();
    fs::write(home.join(AUTH_FILE), r#"{"auth_mode":"apikey"}"#).unwrap();
    assert!(!profile_bindings(&home, &backups).unwrap()[0].active);
    fs::write(home.join(AUTH_FILE), managed_auth).unwrap();

    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"codex_local_access\"\n\n[model_providers.codex_local_access]\nbase_url = \"https://api.example.test/v1\"\n",
        )
        .unwrap();
    let bindings = profile_bindings(&home, &backups).unwrap();
    assert_eq!(bindings.len(), 1);
    assert!(!bindings[0].active);

    fs::remove_dir_all(root).unwrap();
}
#[test]
fn switching_external_account_takeover_to_local_rebases_the_latest_profile() {
    let (root, home, backups) = profile_dirs("external-account-takeover");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"chatgpt\"}").unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "managed-access",
        Some("managed-refresh".into()),
        Some("managed-id".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    attach_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-account",
        &secrets,
    )
    .unwrap();

    let external_config = "model_provider = \"external_provider\"\n\n[model_providers.external_provider]\nname = \"External Provider\"\nbase_url = \"http://127.0.0.1:49976/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n";
    let external_auth = "{\"tokens\":{\"access_token\":\"external\"}}";
    fs::write(home.join(CONFIG_FILE), external_config).unwrap();
    fs::write(home.join(AUTH_FILE), external_auth).unwrap();

    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert_eq!(profile_backup_count(&backups), 1);
    assert!(backup_path(&backups).exists());

    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        external_config
    );
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        external_auth
    );
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn local_gateway_projects_and_syncs_a_bound_oauth_profile() {
    let (root, home, backups) = profile_dirs("local-gateway-bound-oauth");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "bound-access",
        Some("bound-refresh".into()),
        Some("bound-id".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    let binding = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert_eq!(
        binding.bound_oauth_account_id.as_deref(),
        Some("account-local")
    );
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("model_provider = \"zenith_relay_local\""));
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("experimental_bearer_token = \"zlr_key\""));
    let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
    assert!(projected.contains("bound-access"));
    assert!(!projected.contains("zlr_key"));
    let projected_value = serde_json::from_str::<serde_json::Value>(&projected).unwrap();
    assert!(projected_value["OPENAI_API_KEY"].is_null());
    assert_eq!(projected_value["auth_mode"], "chatgpt");
    assert_eq!(projected_value["tokens"]["account_id"], "provider-account");
    DateTime::parse_from_rfc3339(projected_value["last_refresh"].as_str().unwrap()).unwrap();

    let refreshed = TokenSet::new(
        "bound-access-refreshed",
        Some("bound-refresh-next".into()),
        Some("bound-id-next".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    assert!(sync_local_gateway_binding(
        &home,
        &backups,
        "account-local",
        &refreshed,
        "provider-account",
    )
    .unwrap());
    assert!(!sync_local_gateway_binding(
        &home,
        &backups,
        "account-local",
        &refreshed,
        "provider-account",
    )
    .unwrap());
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("bound-access-refreshed"));

    restore_with(&home, &backups, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("model_provider = \"custom\""));
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("original"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn local_gateway_restore_adopts_oauth_rotation_before_switching_to_chatgpt() {
    let (root, home, backups) = profile_dirs("local-gateway-restore-after-oauth-rotation");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let original = TokenSet::new(
        "bound-access",
        Some("bound-refresh".into()),
        Some("bound-id".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &original,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();

    let rotated = TokenSet::new(
        "bound-access-rotated",
        Some("bound-refresh-rotated".into()),
        Some("bound-id-rotated".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        account_auth_content(&rotated, "provider-account").unwrap(),
    )
    .unwrap();

    let update = managed_account_token_update(
        &home,
        &backups,
        "account-local",
        &original,
        "provider-account",
    )
    .unwrap()
    .expect("rotated OAuth token");
    assert_eq!(update.access_token, rotated.access_token());
    sync_local_gateway_binding(
        &home,
        &backups,
        "account-local",
        &rotated,
        "provider-account",
    )
    .unwrap();

    restore_with(&home, &backups, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("original"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn local_gateway_can_replace_bound_oauth_with_local_key() {
    let (root, home, backups) = profile_dirs("local-gateway-remove-oauth-binding");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "bound-access",
        Some("bound-refresh".into()),
        Some("bound-id".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    let binding = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();

    assert_eq!(binding.bound_oauth_account_id, None);
    let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
    assert!(projected.contains("zlr_key"));
    assert!(!projected.contains("bound-access"));
    restore_with(&home, &backups, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("original"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn local_gateway_keeps_api_key_projection_when_bound_oauth_has_no_id_token() {
    let (root, home, backups) = profile_dirs("local-gateway-bound-access-only");
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "bound-access",
        Some("bound-refresh".into()),
        None,
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    let binding = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert_eq!(
        binding.bound_oauth_account_id.as_deref(),
        Some("account-local")
    );
    let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
    assert!(projected.contains("zlr_key"));
    assert!(!projected.contains("bound-access"));
    restore_with(&home, &backups, &secrets).unwrap();
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn oauth_account_restore_preserves_a_fresh_manual_login() {
    let (root, home, backups) = profile_dirs("oauth-fresh-login");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
    attach_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    let auth: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
    assert!(auth["tokens"].get("refresh_token").is_none());
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
    )
    .unwrap();

    let restored = restore_account_with(&home, &backups, &secrets)
        .unwrap()
        .unwrap();
    assert!(!restored.active);
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        "model_provider = \"custom\"\n"
    );
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("fresh"));
    assert!(account_bindings(&backups).unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn oauth_restore_merges_external_toml_edits_and_keeps_new_login() {
    let (root, home, backups) = profile_dirs("oauth-external-config-login");
    let original_config =
        "model_provider = 'custom'\nopenai_base_url = 'https://example.test/v1'\n";
    fs::write(home.join(CONFIG_FILE), original_config).unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
    attach_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-account",
        &secrets,
    )
    .unwrap();
    let changed_config = "model_provider = 'other'\nuser_setting = 'keep'\n";
    let fresh_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh"}}"#;
    fs::write(home.join(CONFIG_FILE), changed_config).unwrap();
    fs::write(home.join(AUTH_FILE), fresh_auth).unwrap();

    restore_account_with(&home, &backups, &secrets).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&restored).as_deref(), Some("other"));
    assert_eq!(restored["user_setting"].as_str(), Some("keep"));
    assert_eq!(
        root_openai_base_url(&restored).as_deref(),
        Some("https://example.test/v1")
    );
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        fresh_auth
    );
    assert!(account_bindings(&backups).unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn oauth_account_bindings_are_isolated_per_profile_path() {
    let (root, first, backups) = profile_dirs("oauth-multi-profile");
    let second = root.join("second-profile");
    fs::create_dir_all(&second).unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
    attach_account_with(
        &first,
        &backups,
        "account-local",
        &tokens,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    attach_account_with(
        &second,
        &backups,
        "account-local",
        &tokens,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    assert_eq!(account_bindings(&backups).unwrap().len(), 2);

    restore_account_with(&first, &backups, &secrets).unwrap();
    let remaining = account_bindings(&backups).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].profile_dir,
        canonical_profile_dir(&second).unwrap().to_string_lossy()
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn sync_default_service_tier_preserves_codex_profile_state() {
    let (root, home, _) = profile_dirs("service-tier");
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = \"custom\"\n\n[desktop]\nappearanceTheme = \"dark\"\n",
    )
    .unwrap();
    fs::write(
        home.join(GLOBAL_STATE_FILE),
        r#"{"other":1,"electron-persisted-atom-state":{"theme":"dark"}}"#,
    )
    .unwrap();

    sync_default_service_tier(&home, DefaultServiceTier::Fast).unwrap();
    let config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(
        config["desktop"][DESKTOP_DEFAULT_SERVICE_TIER_KEY].as_str(),
        Some("priority")
    );
    assert_eq!(
        config[TOP_LEVEL_SERVICE_TIER_KEY].as_str(),
        Some("priority")
    );
    assert_eq!(config["desktop"]["appearanceTheme"].as_str(), Some("dark"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(home.join(GLOBAL_STATE_FILE)).unwrap()).unwrap();
    assert_eq!(state["other"], 1);
    assert_eq!(state[PERSISTED_ATOM_STATE_KEY]["theme"], "dark");
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][DESKTOP_DEFAULT_SERVICE_TIER_KEY],
        "priority"
    );
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][SERVICE_TIER_CHANGED_KEY],
        true
    );

    sync_default_service_tier(&home, DefaultServiceTier::Ultrafast).unwrap();
    let config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(
        config["desktop"][DESKTOP_DEFAULT_SERVICE_TIER_KEY].as_str(),
        Some("ultrafast")
    );
    assert_eq!(
        config[TOP_LEVEL_SERVICE_TIER_KEY].as_str(),
        Some("ultrafast")
    );
    let state: Value =
        serde_json::from_str(&fs::read_to_string(home.join(GLOBAL_STATE_FILE)).unwrap()).unwrap();
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][DESKTOP_DEFAULT_SERVICE_TIER_KEY],
        "ultrafast"
    );

    sync_default_service_tier(&home, DefaultServiceTier::Standard).unwrap();
    let config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert!(config["desktop"]
        .as_table()
        .unwrap()
        .get(DESKTOP_DEFAULT_SERVICE_TIER_KEY)
        .is_none());
    assert_eq!(config[TOP_LEVEL_SERVICE_TIER_KEY].as_str(), Some("default"));
    assert_eq!(config["desktop"]["appearanceTheme"].as_str(), Some("dark"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(home.join(GLOBAL_STATE_FILE)).unwrap()).unwrap();
    assert_eq!(state["other"], 1);
    assert!(state[PERSISTED_ATOM_STATE_KEY][DESKTOP_DEFAULT_SERVICE_TIER_KEY].is_null());
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][SERVICE_TIER_CHANGED_KEY],
        true
    );

    fs::remove_dir_all(root).unwrap();
}

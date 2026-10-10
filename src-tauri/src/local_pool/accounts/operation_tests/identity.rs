use super::*;

#[test]
fn refresh_only_without_explicit_account_id_updates_after_exchange_identity() {
    let parsed = parse_import(r#"{"refresh_token":"refresh-rotated"}"#, None, &[]).unwrap();
    assert!(parsed.items[0].account_id.is_none());
    let mut existing = account_record("account_existing");
    existing.account.label = "My account".into();
    existing.account.token_generation = 7;
    existing.account.enabled = false;
    existing.account.in_pool = false;
    existing.priority = 9;
    existing.remote_location = Some(RemoteAccountLocation {
        server_id: "server-one".into(),
        remote_account_id: "account-remote".into(),
    });
    existing.cooldowns.insert("gpt-test".into(), 900);
    existing.consecutive_failures = 2;
    let resolved = existing.clone();
    let credentials = ImportedCredentialMaterial {
        access_token: "access-rotated".into(),
        agent_identity: None,
        refresh_token: Some("refresh-rotated".into()),
        id_token: None,
        expires_at_ms: Some(60_000),
        email: None,
        phone: None,
        password: None,
        totp_secret: None,
        provider_account_id: Some("provider-private".into()),
        account_id_hints: vec!["provider-private".into()],
        provider_user_id: None,
        organization_id: None,
        plan_type: None,
        subscription_active_until_ms: None,
        account_is_fedramp: false,
        basis_points_headers: None,
        oauth_client_kind: Default::default(),
    }
    .into_stored(&resolved.account.id, 2, 8)
    .unwrap();
    let mut updated = records::new_account_record(
        &credentials,
        AccountAuthMode::ImportedToken,
        vec!["gpt-test".into()],
        0,
        2,
    )
    .unwrap();
    merge_existing_account(&mut updated, Some(&resolved));

    assert_eq!(credentials.local_account_id(), "account_existing");
    assert_eq!(credentials.generation(), 8);
    assert_eq!(updated.account.id, "account_existing");
    assert_eq!(updated.account.label, "My account");
    assert_eq!(updated.account.token_generation, 8);
    assert!(!updated.account.enabled);
    assert!(!updated.account.in_pool);
    assert_eq!(updated.priority, 9);
    assert_eq!(updated.remote_location, existing.remote_location);
    assert!(updated.cooldowns.is_empty());
    assert_eq!(updated.consecutive_failures, 0);
    assert_ne!(
        updated.account.identity.stable_index,
        existing.account.identity.stable_index
    );
}
#[test]
fn provider_identity_hash_matches_import_parser_without_exposing_id() {
    let parsed = parse_import(
            r#"{"account_id":"Provider-Private","chatgpt_user_id":"User-Private","email":"private@example.test","access_token":"access-private"}"#,
            None,
            &[],
        )
        .unwrap();
    let key = provider_identity_key(
        "Provider-Private",
        Some("User-Private"),
        Some("private@example.test"),
        Default::default(),
    );
    assert_eq!(parsed.items[0].identity_key, key);
    assert!(!key.contains("provider"));
}
#[test]
fn account_check_parser_honors_order_and_bounds_identity_values() {
    let payload = serde_json::json!({
        "account_ordering": ["preferred", "fallback"],
        "accounts": {
            "preferred": {"account": {"workspace_id": "preferred-id"}},
            "fallback": {"account": {"workspace_id": "fallback-id"}}
        }
    });

    assert_eq!(
        account_id_from_check_response(&payload).as_deref(),
        Some("preferred-id")
    );
    assert_eq!(
        normalized_profile_account_id("  account-id  ").as_deref(),
        Some("account-id")
    );
    assert!(normalized_profile_account_id("bad\0id").is_none());
    assert!(normalized_profile_account_id(&"x".repeat(513)).is_none());
    assert_eq!(masked_account_identity("provider-1234"), "Account ****1234");
}
#[test]
fn imported_jwt_claims_supply_account_identity_without_serializing_token() {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "email": "private@example.test",
            "exp": 123,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_subscription_active_until": 1_767_225_600,
                "chatgpt_user_id": "user-private",
                "account_id": "account-private"
            }
        })
        .to_string(),
    );
    let token = format!("header.{payload}.signature");
    let identity = imported_identity(Some(&token), Some(&token));
    assert_eq!(
        identity.provider_account_id.as_deref(),
        Some("account-private")
    );
    assert_eq!(identity.provider_user_id.as_deref(), Some("user-private"));
    assert_eq!(identity.plan_type.as_deref(), Some("pro"));
    assert_eq!(
        identity.subscription_active_until_ms,
        Some(1_767_225_600_000)
    );
    assert_eq!(identity.access_expires_at_ms, Some(123_000));
    assert!(!hex::encode(Sha256::digest(token.as_bytes())).contains("private"));
}
#[test]
fn subscription_metadata_adds_expiry_and_normalizes_the_plan_alias() {
    let mut subscription = Subscription {
        plan_type: Some("plus".into()),
        ..Default::default()
    };
    apply_subscription_metadata(
        &mut subscription,
        CodexSubscriptionMetadata {
            account_id: None,
            plan_type: Some("chatgptplusplan".into()),
            active_until_ms: Some(1_787_544_851_000),
        },
        123,
    );

    assert_eq!(subscription.plan_type.as_deref(), Some("plus"));
    assert_eq!(subscription.active_until_ms, Some(1_787_544_851_000));
    assert_eq!(subscription.updated_at_ms, Some(123));
}
#[test]
fn imported_identity_prefers_the_access_token_workspace() {
    let token = |account_id: &str| {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "https://api.openai.com/auth": { "chatgpt_account_id": account_id }
            })
            .to_string(),
        );
        format!("header.{payload}.signature")
    };
    let identity = imported_identity(
        Some(&token("workspace-old")),
        Some(&token("workspace-live")),
    );
    assert_eq!(
        identity.provider_account_id.as_deref(),
        Some("workspace-live")
    );
}
#[tokio::test]
async fn account_check_recovers_an_id_from_an_access_only_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|| async {
            Json(serde_json::json!({
                "account_ordering": ["workspace-private"],
                "accounts": {
                    "workspace-private": {
                        "account": { "workspace_id": "workspace-private" }
                    }
                }
            }))
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let endpoint = Url::parse(&format!("http://{address}/accounts/check")).unwrap();

    let account_id = lookup_import_account_id(
        endpoint,
        "synthetic-access-only-token",
        None,
        Duration::from_secs(2),
    )
    .await
    .unwrap();

    assert_eq!(account_id, "workspace-private");
    server.abort();
}
#[tokio::test]
async fn import_rejects_conflicting_document_and_jwt_account_ids() {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "jwt-account"
            }
        })
        .to_string(),
    );
    let access_token = format!("header.{payload}.signature");
    let mut parsed = parse_import(
        &serde_json::json!({
            "account_id": "document-account",
            "access_token": access_token
        })
        .to_string(),
        None,
        &[],
    )
    .unwrap();

    let result = build_import_credential_material(
        parsed.items.remove(0),
        1,
        None,
        None,
        None,
        2,
        &Url::parse("http://127.0.0.1:1/accounts/check").unwrap(),
    )
    .await;
    assert!(
        result.is_err(),
        "conflicting account identity claims were accepted"
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => return,
    };

    assert_eq!(error.code, "account_identity_claim_conflict");
}
#[tokio::test]
async fn import_rejects_an_authenticated_account_identity_mismatch() {
    let (endpoint, server) = spawn_import_account_check_payload(serde_json::json!({
        "accounts": [{"account": {"id": "authenticated-account"}}]
    }))
    .await;
    let mut parsed = parse_import(
        r#"{"account_id":"claimed-account","access_token":"synthetic-access-token"}"#,
        None,
        &[],
    )
    .unwrap();

    let result =
        build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 2, &endpoint)
            .await;
    assert!(
        result.is_err(),
        "an authenticated account identity mismatch was accepted"
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => return,
    };

    assert_eq!(error.code, "account_identity_mismatch");
    server.abort();
}
#[tokio::test]
async fn import_uses_the_authenticated_account_id_when_the_document_has_none() {
    let (endpoint, server) = spawn_import_account_check_payload(serde_json::json!({
        "account_ordering": ["canonical-account"],
        "accounts": {
            "canonical-account": {
                "account": {"workspace_id": "canonical-account"}
            }
        }
    }))
    .await;
    let mut parsed = parse_import(
        r#"{"access_token":"synthetic-access-only-token"}"#,
        None,
        &[],
    )
    .unwrap();

    let material =
        build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 2, &endpoint)
            .await
            .unwrap();

    assert_eq!(
        material.provider_account_id.as_deref(),
        Some("canonical-account")
    );
    server.abort();
}
#[tokio::test]
async fn import_rejects_an_authenticated_response_without_an_account_id() {
    let (endpoint, server) =
        spawn_import_account_check_payload(serde_json::json!({"accounts": []})).await;
    let mut parsed = parse_import(
        r#"{"access_token":"synthetic-access-only-token"}"#,
        None,
        &[],
    )
    .unwrap();

    let result =
        build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 2, &endpoint)
            .await;
    assert!(
        result.is_err(),
        "an account-check response without an id was accepted"
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => return,
    };

    assert_eq!(error.code, "provider_account_id_missing");
    server.abort();
}
#[tokio::test]
async fn import_keeps_a_claimed_account_when_the_access_token_is_rejected() {
    let (endpoint, server) = spawn_rejected_import_account_check().await;
    let mut parsed = parse_import(
        r#"{"account_id":"claimed-account","email":"member@example.test","access_token":"synthetic-expired-access-token","refresh_token":"synthetic-refresh-token"}"#,
        None,
        &[],
    )
    .unwrap();

    let material =
        build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 2, &endpoint)
            .await
            .unwrap();

    assert_eq!(
        material.provider_account_id.as_deref(),
        Some("claimed-account")
    );
    assert_eq!(material.access_token, "synthetic-expired-access-token");
    assert_eq!(
        material.refresh_token.as_deref(),
        Some("synthetic-refresh-token")
    );
    server.abort();
}
#[tokio::test]
async fn import_still_rejects_a_rejected_token_without_an_account_id() {
    let (endpoint, server) = spawn_rejected_import_account_check().await;
    let mut parsed = parse_import(
        r#"{"access_token":"synthetic-expired-access-token"}"#,
        None,
        &[],
    )
    .unwrap();

    let result =
        build_import_credential_material(parsed.items.remove(0), 1, None, None, None, 2, &endpoint)
            .await;
    assert!(
        result.is_err(),
        "a rejected token without an account id was imported"
    );
    let Err(error) = result else {
        return;
    };

    assert_eq!(error.code, "access_token_rejected");
    server.abort();
}
#[tokio::test]
async fn imported_explicit_email_wins_over_shared_token_email() {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "email": "shared@example.test",
            "https://api.openai.com/auth": {
                "chatgpt_user_id": "shared-user",
                "chatgpt_account_id": "shared-team"
            }
        })
        .to_string(),
    );
    let token = format!("header.{payload}.signature");
    let mut parsed = parse_import(
        &serde_json::json!({
            "email": "member@example.test",
            "access_token": token
        })
        .to_string(),
        None,
        &[],
    )
    .unwrap();
    let (account_check_endpoint, account_check_server) =
        spawn_import_account_check_server("shared-team").await;
    let material = build_import_credential_material(
        parsed.items.remove(0),
        1,
        None,
        None,
        None,
        20,
        &account_check_endpoint,
    )
    .await
    .unwrap();
    assert_eq!(material.email.as_deref(), Some("member@example.test"));
    account_check_server.abort();
}

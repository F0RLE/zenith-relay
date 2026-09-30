use super::*;

#[tokio::test]
async fn batch_import_accepts_portable_bundles_and_confirms_selected_accounts() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let zenith_preview_response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "content": json!({
                "format": "zenith",
                "version": 1,
                "description": "Seller description",
                "accounts": [{
                    "name": "Zenith account",
                    "provider": "openai",
                    "auth": {
                        "type": "oauth",
                        "accessToken": "synthetic-zenith-access",
                        "refreshToken": "synthetic-zenith-refresh",
                        "expiresAt": "2026-08-19T00:00:00Z"
                    },
                    "identity": {
                        "accountId": "synthetic-zenith-account"
                    },
                    "subscription": {
                        "plan": "business",
                        "expiresAt": "2026-09-19T00:00:00Z"
                    }
                }]
            }).to_string()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(zenith_preview_response.status(), StatusCode::CREATED);
    let zenith_preview_text = zenith_preview_response.text().await.unwrap();
    assert!(!zenith_preview_text.contains("synthetic-zenith-access"));
    assert!(!zenith_preview_text.contains("synthetic-zenith-refresh"));
    let zenith_preview: Value = serde_json::from_str(&zenith_preview_text).unwrap();
    assert_eq!(zenith_preview["preview"]["format"], "zenith_v1");
    assert_eq!(
        zenith_preview["preview"]["description"],
        "Seller description"
    );
    assert_eq!(zenith_preview["preview"]["rows"][0]["plan"], "business");
    let missing_refresh = client
        .post(format!(
            "{}/accounts/account_missing/refresh",
            server.origin
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(missing_refresh.status(), StatusCode::NOT_FOUND);
    let second_id_token = jwt(json!({
        "exp": 1_789_084_800,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "synthetic-batch-account-two",
            "chatgpt_plan_type": "business",
            "chatgpt_subscription_active_until": "2026-10-10T00:00:00Z"
        }
    }));
    let content = json!({
        "version": 1,
        "proxies": [{"password": "synthetic-proxy-secret-never-import"}],
        "sources": [{"apiKey": "synthetic-source-secret-never-import"}],
        "accounts": [
            {
                "name": "Portable first",
                "platform": "openai",
                "type": "oauth",
                "credentials": {
                    "access_token": "synthetic-batch-access-one",
                    "refresh_token": "synthetic-batch-refresh-one",
                    "account_id": "synthetic-batch-account-one",
                    "expires_at": "2026-08-10T00:00:00Z",
                    "plan_type": "plus",
                    "subscription_expires_at": "2026-09-10T00:00:00Z"
                },
                "models": ["gpt-test"]
            },
            {
                "name": "Portable second",
                "tokens": {
                    "accessToken": "synthetic-batch-access-two",
                    "refreshToken": "synthetic-batch-refresh-two",
                    "idToken": second_id_token
                },
                "models": ["gpt-test"]
            }
        ]
    })
    .to_string();

    let preview_response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview_response.status(), StatusCode::CREATED);
    let preview_text = preview_response.text().await.unwrap();
    for secret in [
        "synthetic-batch-access-one",
        "synthetic-batch-refresh-one",
        "synthetic-batch-access-two",
        "synthetic-batch-refresh-two",
        "synthetic-proxy-secret-never-import",
        "synthetic-source-secret-never-import",
        &second_id_token,
    ] {
        assert!(!preview_text.contains(secret));
    }
    let preview: Value = serde_json::from_str(&preview_text).unwrap();
    assert_eq!(preview["preview"]["format"], "portable_account_bundle");
    assert_eq!(preview["preview"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(preview["preview"]["rows"][0]["plan"], "plus");
    assert_eq!(preview["preview"]["warnings"][0]["code"], "proxies_ignored");
    assert_eq!(preview["preview"]["warnings"].as_array().unwrap().len(), 1);
    let batch_id = preview["sessionId"].as_str().unwrap();
    let rows = preview["preview"]["rows"].as_array().unwrap();
    let first_item_id = rows[0]["itemId"].as_str().unwrap();
    let second_item_id = rows[1]["itemId"].as_str().unwrap();

    let first_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(
            &json!({"sessionId": batch_id, "selectedItemIds": [first_item_id], "addToPool": true}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first_confirm["sessionId"], batch_id);
    assert_eq!(first_confirm["results"][0]["status"], "succeeded");
    assert_eq!(first_confirm["results"][0]["created"], true);
    assert_eq!(server.state.store.accounts().unwrap().len(), 1);
    let first_account = server.state.store.accounts().unwrap().remove(0);
    assert_eq!(first_confirm["results"][0]["accountId"], first_account.id);
    assert!(first_account.in_pool);
    assert_eq!(
        first_account.subscription.plan_type.as_deref(),
        Some("plus")
    );
    assert!(first_account.subscription.active_until_ms.is_some());
    let credential: Value = serde_json::from_str(
        &server
            .state
            .vault
            .load(&first_account.secret_ref)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(credential["expiresAtMs"]
        .as_u64()
        .is_some_and(|value| value > 1_000_000_000_000));

    let existing_preview: Value = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth(&server.state.config.management_token)
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(existing_preview["preview"]["rows"][0]["existing"], true);
    let existing_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth(&server.state.config.management_token)
        .json(&json!({
            "sessionId": existing_preview["sessionId"],
            "selectedItemIds": [existing_preview["preview"]["rows"][0]["itemId"]],
            "addToPool": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(existing_confirm["results"][0]["status"], "succeeded");
    assert_eq!(existing_confirm["results"][0]["created"], false);
    assert_eq!(server.state.store.accounts().unwrap().len(), 1);

    let second_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": batch_id, "selectedItemIds": [second_item_id]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        second_confirm["results"][0]["status"], "succeeded",
        "{second_confirm}"
    );
    assert_eq!(second_confirm["results"][0]["created"], true);
    assert_eq!(server.state.store.accounts().unwrap().len(), 2);
    let second_account = server
        .state
        .store
        .accounts()
        .unwrap()
        .into_iter()
        .find(|account| account.label == "Portable second")
        .unwrap();
    assert!(!second_account.in_pool);
    assert_eq!(
        second_account.subscription.plan_type.as_deref(),
        Some("business")
    );
    assert_eq!(
        second_account.subscription.active_until_ms,
        Some(1_791_590_400_000)
    );

    let database = std::fs::read(root.path().join("relay.sqlite")).unwrap();
    let database = String::from_utf8_lossy(&database);
    assert!(!database.contains("synthetic-proxy-secret-never-import"));
    assert!(!database.contains("synthetic-source-secret-never-import"));
    server.task.abort();
}

fn jwt(payload: Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("{header}.{payload}.synthetic-signature")
}

#[tokio::test]
async fn batch_import_accepts_multiple_documents_and_confirms_every_selected_account() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let documents = (1..=3)
        .map(|index| {
            json!({
                "name": format!("Document {index}"),
                "credentials": {
                    "access_token": format!("synthetic-document-access-{index}"),
                    "refresh_token": format!("synthetic-document-refresh-{index}"),
                    "chatgpt_account_id": format!("synthetic-document-account-{index}")
                },
                "models": ["gpt-test"]
            })
            .to_string()
        })
        .collect::<Vec<_>>();

    let preview_response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"documents": documents}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview_response.status(), StatusCode::CREATED);
    let preview_text = preview_response.text().await.unwrap();
    assert!(!preview_text.contains("synthetic-document-access"));
    assert!(!preview_text.contains("synthetic-document-refresh"));
    let preview: Value = serde_json::from_str(&preview_text).unwrap();
    assert_eq!(preview["preview"]["format"], "json_array");
    let rows = preview["preview"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row["defaultSelected"] == true));
    let selected_item_ids = rows
        .iter()
        .map(|row| row["itemId"].as_str().unwrap())
        .collect::<Vec<_>>();

    let confirmed: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": preview["sessionId"],
            "selectedItemIds": selected_item_ids
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(confirmed["results"].as_array().unwrap().len(), 3);
    assert!(confirmed["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|result| result["status"] == "succeeded"));
    let accounts = server.state.store.accounts().unwrap();
    assert_eq!(accounts.len(), 3);
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.secret_ref.as_str())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert!(accounts.iter().all(|account| server
        .state
        .vault
        .load(&account.secret_ref)
        .unwrap()
        .is_some()));
    server.task.abort();
}

#[tokio::test]
async fn batch_import_accepts_agent_identity_and_keeps_it_in_the_vault() {
    const PRIVATE_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let content = json!({
        "type": "sub2api-data",
        "version": 1,
        "accounts": [{
            "name": "Agent account",
            "credentials": {
                "auth_mode": "agentIdentity",
                "agent_private_key": PRIVATE_KEY,
                "agent_runtime_id": "runtime-test",
                "task_id": "task-test",
                "chatgpt_account_id": "account-test"
            },
            "models": ["gpt-test"]
        }]
    })
    .to_string();
    let response = client
        .post(format!("{}/accounts/import/batch/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let text = response.text().await.unwrap();
    assert!(!text.contains(PRIVATE_KEY));
    let preview: Value = serde_json::from_str(&text).unwrap();
    let item_id = preview["preview"]["rows"][0]["itemId"].as_str().unwrap();
    let confirmed: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": preview["sessionId"],
            "selectedItemIds": [item_id],
            "addToPool": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(confirmed["results"][0]["status"], "succeeded");
    let account = server.state.store.accounts().unwrap().remove(0);
    let stored = server
        .state
        .vault
        .load(&account.secret_ref)
        .unwrap()
        .unwrap();
    assert!(stored.contains(PRIVATE_KEY));
    assert!(!std::fs::read(root.path().join("relay.sqlite"))
        .unwrap()
        .windows(PRIVATE_KEY.len())
        .any(|window| window == PRIVATE_KEY.as_bytes()));
    server.task.abort();
}

#[tokio::test]
async fn batch_import_handles_arrays_json_lines_invalid_rows_and_duplicates() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let valid = json!({
        "label": "Array account",
        "accessToken": "synthetic-array-access",
        "refreshToken": "synthetic-array-refresh",
        "chatgptAccountId": "synthetic-array-account",
        "models": ["gpt-test"]
    });
    let preview = batch_preview(
        &client,
        &server.origin,
        json!([
            valid,
            {"name": "invalid-row-marker"},
            {
                "name": "synthetic-label-secret",
                "accessToken": "synthetic-label-secret",
                "chatgptAccountId": "synthetic-label-account",
                "planType": "synthetic-label-secret"
            }
        ])
        .to_string(),
    )
    .await;
    assert_eq!(preview["preview"]["format"], "json_array");
    assert_eq!(preview["preview"]["rows"][0]["status"], "ready");
    assert_eq!(preview["preview"]["rows"][1]["status"], "invalid");
    assert_eq!(
        preview["preview"]["rows"][1]["error"]["code"],
        "missing_credentials"
    );
    assert!(!preview.to_string().contains("invalid-row-marker"));
    assert!(!preview.to_string().contains("synthetic-label-secret"));
    assert_eq!(preview["preview"]["rows"][2]["label"], "synt...ount");
    assert!(preview["preview"]["rows"][2]["plan"].is_null());

    let batch_id = preview["sessionId"].as_str().unwrap();
    let item_id = preview["preview"]["rows"][0]["itemId"].as_str().unwrap();
    let confirmed = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": batch_id, "selectedItemIds": [item_id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);

    let mut configured = server.state.store.accounts().unwrap().remove(0);
    configured.label = "Server display name".into();
    configured.enabled = false;
    configured.draining = true;
    configured.models = vec!["gpt-server".into()];
    configured.allowed_models = vec!["gpt-server".into()];
    configured.excluded_models = vec!["gpt-blocked".into()];
    configured.priority = 42;
    configured.weight = 7;
    server.state.store.save_account(&configured).unwrap();

    let duplicate = batch_preview(
        &client,
        &server.origin,
        json!({
            "label": "Duplicate account",
            "accessToken": "synthetic-duplicate-access",
            "chatgptAccountId": "synthetic-array-account"
        })
        .to_string(),
    )
    .await;
    assert_eq!(duplicate["preview"]["rows"][0]["status"], "existing");
    assert_eq!(duplicate["preview"]["rows"][0]["defaultSelected"], false);
    let duplicate_confirm: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": duplicate["sessionId"],
            "selectedItemIds": [duplicate["preview"]["rows"][0]["itemId"]]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate_confirm["results"][0]["status"], "succeeded");
    let preserved = server.state.store.accounts().unwrap().remove(0);
    assert_eq!(preserved.label, "Server display name");
    assert!(!preserved.enabled);
    assert!(preserved.draining);
    assert_eq!(preserved.models, vec!["gpt-server"]);
    assert_eq!(preserved.allowed_models, vec!["gpt-server"]);
    assert_eq!(preserved.excluded_models, vec!["gpt-blocked"]);
    assert_eq!(preserved.priority, 42);
    assert_eq!(preserved.weight, 7);

    let json_lines = [
        json!({"label":"Line one","accessToken":"synthetic-line-access-one","chatgptAccountId":"synthetic-line-account-one"}).to_string(),
        json!({"label":"Line two","accessToken":"synthetic-line-access-two","chatgptAccountId":"synthetic-line-account-two"}).to_string(),
    ]
    .join("\n");
    let lines_preview = batch_preview(&client, &server.origin, json_lines).await;
    assert_eq!(lines_preview["preview"]["format"], "json_lines");
    assert_eq!(
        lines_preview["preview"]["rows"].as_array().unwrap().len(),
        2
    );
    server.task.abort();
}

#[tokio::test]
async fn batch_import_enforces_size_count_depth_and_batch_ownership() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let oversized = "x".repeat(4 * 1024 * 1024 + 1);
    assert_batch_error(&client, &server.origin, oversized, "import_too_large").await;
    let too_many = Value::Array((0..1_025).map(|_| json!({})).collect()).to_string();
    assert_batch_error(&client, &server.origin, too_many, "import_item_count").await;
    let mut deep = json!({});
    for _ in 0..34 {
        deep = json!({"nested": deep});
    }
    assert_batch_error(&client, &server.origin, deep.to_string(), "import_too_deep").await;

    let first = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-owned-one","chatgptAccountId":"synthetic-owned-account-one"}).to_string(),
    )
    .await;
    let second = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-owned-two","chatgptAccountId":"synthetic-owned-account-two"}).to_string(),
    )
    .await;
    let response: Value = client
        .post(format!("{}/accounts/import/batch/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "sessionId": first["sessionId"],
            "selectedItemIds": [second["preview"]["rows"][0]["itemId"]]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["results"][0]["status"], "failed");
    assert_eq!(response["results"][0]["error"]["code"], "import_not_found");
    assert!(server.state.store.accounts().unwrap().is_empty());

    let abandoned = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-abandoned","chatgptAccountId":"synthetic-abandoned-account"}).to_string(),
    )
    .await;
    let abandoned_id = abandoned["preview"]["rows"][0]["itemId"].as_str().unwrap();
    let mut pending = server
        .state
        .store
        .pending_import(abandoned_id)
        .unwrap()
        .unwrap();
    pending.created_at_ms = 1;
    let abandoned_secret_ref = pending.secret_ref.clone();
    server.state.store.save_pending_import(&pending).unwrap();
    assert!(server
        .state
        .vault
        .load(&abandoned_secret_ref)
        .unwrap()
        .is_some());
    let _ = batch_preview(
        &client,
        &server.origin,
        json!({"accessToken":"synthetic-cleanup-trigger","chatgptAccountId":"synthetic-cleanup-trigger-account"}).to_string(),
    )
    .await;
    assert!(server
        .state
        .store
        .pending_import(abandoned_id)
        .unwrap()
        .is_none());
    assert!(server
        .state
        .vault
        .load(&abandoned_secret_ref)
        .unwrap()
        .is_none());
    server.task.abort();
}

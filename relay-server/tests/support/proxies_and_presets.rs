use super::*;

#[tokio::test]
async fn server_account_proxies_support_common_override_bulk_and_redaction() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let (common_address, common_hits, common_task) = spawn_account_proxy("common-proxy").await;
    let (account_address, account_hits, account_task) = spawn_account_proxy("account-proxy").await;
    let client = reqwest::Client::new();
    let common_secret = format!("common-user:common-pass@{common_address}");
    let account_secret = format!("account-user:account-pass@{account_address}");

    let common_state: Value = client
        .post(format!("{}/proxies/common", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": common_secret}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(common_state["gateway"]["commonProxyConfigured"], true);
    assert_eq!(common_state["gateway"]["commonProxyAvailable"], true);
    assert!(!common_state.to_string().contains("common-pass"));

    let preview: Value = client
        .post(format!("{}/accounts/import/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Proxy account",
            "accessToken": "synthetic-proxy-access-token",
            "expiresAtMs": 4_000_000_000_000_u64,
            "chatgptAccountId": "synthetic-proxy-account-id",
            "responsesUrl": "http://127.0.0.1:9/account/responses",
            "models": ["gpt-proxy-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let account_id = preview["accountId"].as_str().unwrap().to_string();
    let confirmed: Value = client
        .post(format!("{}/accounts/import/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": preview["sessionId"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(confirmed["proxyMode"], "common");
    assert_eq!(confirmed["proxyAvailable"], true);
    let membership = client
        .post(format!("{}/pool/members", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"accountIds": [account_id], "inPool": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);

    let profile: Value = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pool_key = profile["secret"].as_str().unwrap();

    let first = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"common proxy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert!(first.text().await.unwrap().contains("common-proxy"));
    assert_eq!(common_hits.load(Ordering::SeqCst), 1);

    let account_state: Value = client
        .post(format!("{}/accounts/{account_id}/proxy", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": account_secret}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(account_state["proxyMode"], "account");
    assert!(!account_state.to_string().contains("account-pass"));

    let second = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"account proxy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert!(second.text().await.unwrap().contains("account-proxy"));
    assert_eq!(account_hits.load(Ordering::SeqCst), 1);

    let replacement_preview: Value = client
        .post(format!("{}/accounts/import/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Proxy account refreshed",
            "accessToken": "synthetic-proxy-access-token-refreshed",
            "expiresAtMs": 4_000_000_100_000_u64,
            "chatgptAccountId": "synthetic-proxy-account-id",
            "responsesUrl": "http://127.0.0.1:9/account/responses",
            "models": ["gpt-proxy-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replacement_preview["accountId"], account_id);
    let replacement: Value = client
        .post(format!("{}/accounts/import/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": replacement_preview["sessionId"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replacement["proxyMode"], "account");
    assert!(!replacement.to_string().contains("account-pass"));

    let bulk: Value = client
        .post(format!("{}/accounts/proxies/assign", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "accountIds": [account_id],
            "proxyUrls": [account_secret, "unused:unused@127.0.0.1:9999"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bulk, json!({"assigned": 1, "unused": 1}));

    let inherited: Value = client
        .post(format!("{}/accounts/{account_id}/proxy", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(inherited["proxyMode"], "common");
    let third = client
        .post(format!("{}/v1/responses", server.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"inherited proxy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::OK);
    assert!(third.text().await.unwrap().contains("common-proxy"));
    assert_eq!(common_hits.load(Ordering::SeqCst), 2);

    let state_text = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for secret in ["common-pass", "account-pass", "unused:unused"] {
        assert!(!state_text.contains(secret));
    }
    let database_bytes = std::fs::read(root.path().join("relay.sqlite")).unwrap();
    let database = String::from_utf8_lossy(&database_bytes);
    let vault_bytes = std::fs::read(root.path().join("vault").join("secrets.enc")).unwrap();
    let vault = String::from_utf8_lossy(&vault_bytes);
    for secret in ["common-pass", "account-pass", "unused:unused"] {
        assert!(!database.contains(secret));
        assert!(!vault.contains(secret));
    }

    let common_proxy = server
        .state
        .store
        .proxy(&server.state.store.common_proxy_id().unwrap().unwrap())
        .unwrap()
        .unwrap();
    server.state.vault.delete(&common_proxy.secret_ref).unwrap();
    server.task.abort();
    let _ = server.task.await;
    drop(server.state);

    let recovered = spawn_server(root.path()).await;
    let recovery_state: Value = client
        .get(format!("{}/state", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovery_state["gateway"]["commonProxyConfigured"], true);
    assert_eq!(recovery_state["gateway"]["commonProxyAvailable"], false);
    assert_eq!(recovery_state["gateway"]["running"], false);
    assert_eq!(recovery_state["accounts"][0]["proxyMode"], "common");
    assert_eq!(recovery_state["accounts"][0]["proxyAvailable"], false);

    let repaired_state: Value = client
        .post(format!("{}/proxies/common", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": common_secret}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repaired_state["gateway"]["commonProxyAvailable"], true);
    assert_eq!(repaired_state["gateway"]["running"], true);

    let strict_state: Value = client
        .post(format!("{}/proxies/policy", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"required": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(strict_state["gateway"]["accountProxyRequired"], true);
    let blocked_state: Value = client
        .post(format!("{}/proxies/common", recovered.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(blocked_state["gateway"]["commonProxyConfigured"], false);
    assert_eq!(blocked_state["gateway"]["accountProxyRequired"], true);
    assert_eq!(blocked_state["accounts"][0]["proxyMode"], "direct");
    assert_eq!(blocked_state["accounts"][0]["proxyAvailable"], false);
    let blocked_request = client
        .post(format!("{}/v1/responses", recovered.origin))
        .bearer_auth(pool_key)
        .json(&json!({"model":"gpt-proxy-test","input":"must not use direct egress"}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_request.status(), StatusCode::SERVICE_UNAVAILABLE);

    recovered.task.abort();
    common_task.abort();
    account_task.abort();
}

#[tokio::test]
async fn configuration_presets_preview_apply_reject_stale_and_exclude_secrets() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Preset source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let source_id = source["id"].as_str().unwrap();
    let proxy_secret = "http://preset-user:preset-pass@127.0.0.1:9";
    let proxy_state = client
        .post(format!("{}/proxies/common", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": proxy_secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(proxy_state.status(), StatusCode::OK);

    let account_preview: Value = client
        .post(format!("{}/accounts/import/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "label": "Preset account",
            "accessToken": "synthetic-preset-access-token",
            "expiresAtMs": 4_000_000_000_000_u64,
            "chatgptAccountId": "synthetic-preset-account-id",
            "responsesUrl": format!("{upstream}/v1/responses"),
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let account_id = account_preview["accountId"].as_str().unwrap();
    let account_confirm = client
        .post(format!("{}/accounts/import/confirm", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"sessionId": account_preview["sessionId"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(account_confirm.status(), StatusCode::OK);
    let account_proxy = client
        .post(format!("{}/accounts/{account_id}/proxy", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"proxyUrl": proxy_secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(account_proxy.status(), StatusCode::OK);

    let document_response = client
        .get(format!("{}/configuration/preset", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(document_response.status(), StatusCode::OK);
    let document_text = document_response.text().await.unwrap();
    for excluded in [
        "synthetic-upstream-api-key",
        "synthetic-preset-access-token",
        "preset-pass",
        "managementToken",
        "profileCredential",
        "vault",
        "usage",
        "publicBaseUrl",
    ] {
        assert!(!document_text.contains(excluded), "{excluded}");
    }
    let document: Value = serde_json::from_str(&document_text).unwrap();
    assert_eq!(document["preset"]["format"], "zenith-relay-configuration");
    assert_eq!(document["preset"]["schemaVersion"], 6);
    assert!(document["revision"]
        .as_str()
        .is_some_and(|revision| revision.starts_with("cfg_")));
    assert!(document["preset"]["settings"]["quota"]["commonProxyId"]
        .as_str()
        .is_some_and(|proxy_id| proxy_id.starts_with("proxy_")));

    let mut preset = document["preset"].clone();
    let source_rule = preset["settings"]["sources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|rule| rule["id"] == source_id)
        .unwrap();
    source_rule["inPool"] = json!(true);
    source_rule["priority"] = json!(7);
    source_rule["id"] = json!("source_local_record");
    let account_rule = preset["settings"]["accounts"]
        .as_array_mut()
        .unwrap()
        .first_mut()
        .unwrap();
    account_rule["id"] = json!("account_local_record");
    account_rule["inPool"] = json!(true);
    account_rule["priority"] = json!(9);
    preset["settings"]["routing"]["maxRetryCandidates"] = json!(4);
    preset["settings"]["routing"]["poolRouting"] = json!({
        "version": 2,
        "mode": "round_robin",
        "members": [
            {"kind": "source", "id": "source_local_record", "weight": 3, "maxConcurrency": 2},
            {"kind": "account", "id": "account_local_record", "weight": 1, "maxConcurrency": 1}
        ]
    });
    preset["settings"]["hiddenModels"] = json!(["gpt-test"]);

    let preview: Value = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": preset}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let paths = preview["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(preview["preset"]["settings"]["sources"][0]["id"], source_id);
    assert_eq!(
        preview["preset"]["settings"]["accounts"][0]["id"],
        account_id
    );
    assert!(paths.contains("/sources/0/inPool"));
    assert!(paths.contains("/sources/0/priority"));
    assert!(paths.contains("/accounts/0/inPool"));
    assert!(paths.contains("/accounts/0/priority"));
    assert!(paths.contains("/routing/maxRetryCandidates"));
    assert!(paths.contains("/hiddenModels"));
    let priority_change = preview["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == "/sources/0/priority")
        .unwrap();
    assert_eq!(priority_change["before"], 0);
    assert_eq!(priority_change["after"], 7);

    let routing_change = client
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"maxRetryCandidates": 5, "routingStrategy": "adaptive"}))
        .send()
        .await
        .unwrap();
    assert_eq!(routing_change.status(), StatusCode::OK);
    let stale = client
        .post(format!("{}/configuration/preset/apply", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "baseRevision": preview["baseRevision"],
            "preset": preview["preset"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let stale_body: Value = stale.json().await.unwrap();
    assert_eq!(stale_body["error"]["code"], "configuration_revision_stale");
    let unchanged: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged["gateway"]["maxRetryCandidates"], 5);
    assert_eq!(unchanged["sources"][0]["priority"], 0);
    assert_eq!(unchanged["sources"][0]["inPool"], false);

    let fresh_preview: Value = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": preview["preset"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let applied: Value = client
        .post(format!("{}/configuration/preset/apply", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "baseRevision": fresh_preview["baseRevision"],
            "preset": fresh_preview["preset"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(applied["previousRevision"], applied["revision"]);
    let applied_state: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(applied_state["configurationRevision"], applied["revision"]);
    assert_eq!(applied_state["gateway"]["maxRetryCandidates"], 4);
    let expected_pool_routing = json!({
        "version": 2,
        "mode": "round_robin",
        "members": [
            {"kind": "source", "id": source_id, "weight": 3, "maxConcurrency": 2},
            {"kind": "account", "id": account_id, "weight": 1, "maxConcurrency": 1}
        ]
    });
    assert_eq!(
        applied_state["gateway"]["poolRouting"],
        expected_pool_routing
    );
    let mut stale_expected_pool_routing =
        document["preset"]["settings"]["routing"]["poolRouting"].clone();
    stale_expected_pool_routing["mode"] = json!("automatic");
    let stale_routing = client
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "poolRouting": expected_pool_routing,
            "expectedPoolRouting": stale_expected_pool_routing,
            "maxRetryCandidates": 8
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stale_routing.status(), StatusCode::BAD_REQUEST);
    let rejected: Value = stale_routing.json().await.unwrap();
    assert_eq!(rejected["error"]["code"], "pool_routing_conflict");
    assert_eq!(applied_state["sources"][0]["priority"], 7);
    assert_eq!(applied_state["sources"][0]["inPool"], true);
    assert_eq!(applied_state["accounts"][0]["priority"], 9);
    assert_eq!(applied_state["accounts"][0]["inPool"], true);
    assert_eq!(
        applied_state["accounts"][0]["proxyId"],
        document["preset"]["settings"]["quota"]["commonProxyId"]
    );
    assert_eq!(applied_state["gateway"]["visibleModelIds"], json!([]));

    let mut unsupported_schema = fresh_preview["preset"].clone();
    unsupported_schema["schemaVersion"] =
        json!(zenith_relay_core::protocol::CONFIGURATION_PRESET_SCHEMA_VERSION + 1);
    let unsupported = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": unsupported_schema}))
        .send()
        .await
        .unwrap();
    assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);

    let mut unknown_field = fresh_preview["preset"].clone();
    unknown_field["settings"]["unexpected"] = json!(true);
    let unknown = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": unknown_field}))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut missing = fresh_preview["preset"].clone();
    missing["settings"]["sources"][0]["id"] = json!("source_missing");
    missing["settings"]["sources"][0]["baseUrl"] = json!("https://missing.invalid/v1");
    let missing_response = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": missing}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let missing_body: Value = missing_response.json().await.unwrap();
    assert_eq!(
        missing_body["error"]["code"],
        "configuration_reference_missing"
    );
    let after_failures: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after_failures["configurationRevision"], applied["revision"]);

    server.task.abort();
    let _ = server.task.await;
    drop(server.state);
    let restarted = spawn_server(root.path()).await;
    let restarted_state: Value = client
        .get(format!("{}/state", restarted.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        restarted_state["configurationRevision"],
        applied["revision"]
    );
    assert_eq!(restarted_state["gateway"]["maxRetryCandidates"], 4);
    assert_eq!(
        restarted_state["gateway"]["poolRouting"],
        expected_pool_routing
    );
    assert_eq!(restarted_state["sources"][0]["priority"], 7);
    assert_eq!(restarted_state["accounts"][0]["priority"], 9);
    assert_eq!(restarted_state["accounts"][0]["inPool"], true);
    restarted.task.abort();
    upstream_task.abort();
}

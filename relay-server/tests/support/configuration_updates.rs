use super::*;

#[tokio::test]
async fn model_order_reset_clears_persisted_override_and_survives_restart() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    add_rebuild_failing_source(&server.state);
    let mut source = server.state.store.sources().unwrap().remove(0);
    source.weight = 1;
    source.models = vec!["alpha-model".into(), "beta-model".into()];
    server.state.store.save_source(&source).unwrap();
    server.state.rebuild_runtime().await.unwrap();
    let original = server.state.snapshot().unwrap().gateway.models;
    let original_ids = original
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    let reversed = original_ids.iter().rev().cloned().collect::<Vec<_>>();
    assert_eq!(original_ids.len(), 2);
    let client = reqwest::Client::new();
    for (requested, expected) in [(&reversed, &reversed), (&Vec::new(), &original_ids)] {
        let response = client
            .post(format!("{}/models/order", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({ "modelIds": requested }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        let ids = body["gateway"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids, *expected);
        assert_eq!(
            server.state.store.model_display_order().unwrap(),
            *requested
        );
    }
    server.task.abort();
    drop(server);
    let reopened = spawn_server(root.path()).await;
    assert!(reopened
        .state
        .store
        .model_display_order()
        .unwrap()
        .is_empty());
    let ids = reopened
        .state
        .snapshot()
        .unwrap()
        .gateway
        .models
        .into_iter()
        .map(|model| model.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, original_ids);
    reopened.task.abort();
}

#[tokio::test]
async fn model_order_edits_use_complete_inventory_and_reject_invalid_changes_atomically() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    add_rebuild_failing_source(&server.state);
    let mut source = server.state.store.sources().unwrap().remove(0);
    source.enabled = false;
    source.weight = 1;
    source.models = vec!["Alpha".into(), "Beta".into(), "new-model".into()];
    source.excluded_models = vec!["Beta".into()];
    source.protocol_bindings = vec![zenith_relay_core::SourceProtocolBinding::legacy(
        WireApi::Responses,
        &["binding-model".into()],
    )];
    server.state.store.save_source(&source).unwrap();
    let mut outside = source.clone();
    outside.id = "outside-source".into();
    outside.secret_ref = "source:outside".into();
    outside.in_pool = false;
    outside.models = vec!["outside-model".into()];
    outside.protocol_bindings.clear();
    server.state.store.save_source(&outside).unwrap();
    let saved = vec!["stale-model".into(), "Beta".into(), "Alpha".into()];
    server
        .state
        .store
        .set_model_display_order(saved.clone())
        .unwrap();

    let client = reqwest::Client::new();
    for (requested, status, code) in [
        (vec!["missing"], StatusCode::NOT_FOUND, "model_not_found"),
        (
            vec!["outside-model"],
            StatusCode::NOT_FOUND,
            "model_not_found",
        ),
        (
            vec!["Alpha", " ALPHA "],
            StatusCode::BAD_REQUEST,
            "model_order_invalid",
        ),
    ] {
        let response = client
            .post(format!("{}/models/order", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({ "modelIds": requested }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], code);
        assert_eq!(server.state.store.model_display_order().unwrap(), saved);
    }

    let response = client
        .post(format!("{}/models/order", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({ "modelIds": ["binding-model", " alpha "] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        server.state.store.model_display_order().unwrap(),
        ["binding-model", "Alpha", "Beta", "new-model"]
    );
    server.task.abort();
}

#[tokio::test]
async fn gateway_start_rolls_back_enabled_flag_when_runtime_rebuild_fails() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    server.state.store.set_gateway_enabled(false).unwrap();
    add_rebuild_failing_source(&server.state);

    let response = reqwest::Client::new()
        .post(format!("{}/gateway/start", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap();

    assert!(!response.status().is_success());
    assert!(!server.state.store.gateway_enabled().unwrap());
    assert!(server.state.runtime().unwrap().is_none());
    server.task.abort();
}

#[tokio::test]
async fn routing_policy_hot_update_does_not_rebuild_unrelated_invalid_source() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    add_rebuild_failing_source(&server.state);

    let response = reqwest::Client::new()
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({ "maxRetryCandidates": 5 }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["gateway"]["maxRetryCandidates"], 5);
    assert_eq!(
        server
            .state
            .store
            .routing_policy()
            .unwrap()
            .max_retry_candidates,
        5
    );
    server.task.abort();
}

#[tokio::test]
async fn legacy_basis_points_switch_keeps_each_connections_native_transport() {
    let root = TempDir::new().unwrap();
    let (upstream, upstream_task) = spawn_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let management_key = "synthetic-management-token-value";
    let mut connections = Vec::new();
    for (kind, basis_points) in [("codex", false), ("excel_bps", true)] {
        let response = client
            .post(format!("{}/accounts/import/preview", server.origin))
            .bearer_auth(management_key)
            .json(&json!({
                "label": "OAuth account", "oauthClientKind": kind,
                "accessToken": "synthetic-access-token", "refreshToken": "synthetic-refresh-token",
                "expiresAtMs": 4_000_000_000_000_u64,
                "chatgptAccountId": "synthetic-chatgpt-account-id",
                "responsesUrl": format!("{upstream}/account/responses"), "models": ["gpt-test"]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let preview: Value = response.json().await.unwrap();
        let response = client
            .post(format!("{}/accounts/import/confirm", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"sessionId": preview["sessionId"], "addToPool": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let account: Value = response.json().await.unwrap();
        connections.push((account["id"].as_str().unwrap().to_owned(), basis_points));
    }
    assert_ne!(connections[0].0, connections[1].0);
    let runtime = server.state.runtime().unwrap().unwrap();
    for enabled in [true, false] {
        let response = client
            .post(format!("{}/routing/settings", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"maxRetryCandidates": 4, "basisPointsEnabled": enabled}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot: Value = response.json().await.unwrap();
        assert_eq!(snapshot["gateway"]["basisPointsEnabled"], false);
        for (account_id, basis_points) in &connections {
            let account = snapshot["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|account| account["id"].as_str() == Some(account_id.as_str()))
                .unwrap();
            assert_eq!(account["basisPointsAvailable"], *basis_points);
            assert_eq!(account["basisPointsEnabled"], *basis_points);
        }
        assert!(Arc::ptr_eq(
            &runtime,
            &server.state.runtime().unwrap().unwrap()
        ));
    }
    server.task.abort();
    upstream_task.abort();
}

use super::*;

#[tokio::test]
async fn user_source_lifecycle_rotates_the_server_secret_and_routes_with_it() {
    let root = TempDir::new().unwrap();
    let (upstream, observed, upstream_task) = spawn_source_lifecycle_upstream().await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let management_key = "synthetic-management-token-value";
    let first_key = "synthetic-source-key-v1";
    let second_key = "synthetic-source-key-v2";

    let created_response = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "name": "Lifecycle source",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": first_key,
            "wireApi": "responses",
            "models": []
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created_response.status(), StatusCode::CREATED);
    let created_text = created_response.text().await.unwrap();
    assert!(!created_text.contains(first_key));
    let created: Value = serde_json::from_str(&created_text).unwrap();
    let source_id = created["id"].as_str().unwrap();
    assert_eq!(created["models"], json!(["gpt-source-lifecycle"]));

    let second_created: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "name": "Lifecycle backup",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": first_key,
            "wireApi": "responses",
            "models": []
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_source_id = second_created["id"].as_str().unwrap();
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"sourceIds": [source_id, second_source_id], "inPool": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{}/gateway/start", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let source_runtime = server.state.runtime().unwrap().unwrap();
    let retry_at_ms = 4_000_000_000_000_u64;
    source_runtime.set_candidate_cooldown(source_id, "gpt-source-lifecycle", retry_at_ms);
    let mut source_priorities = serde_json::Map::new();
    source_priorities.insert(source_id.to_string(), json!(2));
    source_priorities.insert(second_source_id.to_string(), json!(1));
    let ordered: Value = client
        .patch(format!("{}/sources/{source_id}", server.origin))
        .bearer_auth(management_key)
        .json(&json!({
            "priority": 2,
            "sourcePriorities": source_priorities,
            "allowedModels": ["gpt-source-lifecycle"],
            "recoveryDelaySeconds": 15
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ordered["priority"], json!(2));
    let updated_source_runtime = server.state.runtime().unwrap().unwrap();
    assert!(Arc::ptr_eq(&source_runtime, &updated_source_runtime));
    assert_eq!(
        updated_source_runtime
            .candidate_runtime_order()
            .into_iter()
            .find(|candidate| candidate.candidate_id == source_id)
            .and_then(|candidate| candidate.next_retry_at_ms),
        Some(retry_at_ms)
    );
    let listed: Vec<Value> = client
        .get(format!("{}/sources", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        listed
            .iter()
            .find(|source| source["id"] == source_id)
            .unwrap()["priority"],
        json!(2)
    );
    assert_eq!(
        listed
            .iter()
            .find(|source| source["id"] == second_source_id)
            .unwrap()["priority"],
        json!(1)
    );

    let stats_response = client
        .get(format!("{}/sources/{source_id}/stats", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap();
    assert_eq!(stats_response.status(), StatusCode::OK);
    let stats_text = stats_response.text().await.unwrap();
    assert!(!stats_text.contains(first_key));
    assert_eq!(
        serde_json::from_str::<Value>(&stats_text).unwrap(),
        json!({
            "provider": "unsupported",
            "balanceMicroUsd": null,
            "spentMicroUsd": null,
            "requests": null,
            "totalTokens": null,
            "status": "unsupported",
            "balanceKind": "wallet",
            "balanceUnlimited": false,
            "amounts": []
        })
    );

    let disabled: Value = client
        .patch(format!("{}/sources/{source_id}", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"name":"Lifecycle source edited","enabled":false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(disabled["name"], "Lifecycle source edited");
    assert_eq!(disabled["enabled"], false);
    assert!(Arc::ptr_eq(
        &source_runtime,
        &server.state.runtime().unwrap().unwrap()
    ));

    let rotated_response = client
        .patch(format!("{}/sources/{source_id}", server.origin))
        .bearer_auth(management_key)
        .json(&json!({"apiKey":second_key,"enabled":true}))
        .send()
        .await
        .unwrap();
    let rotated_text = rotated_response.text().await.unwrap();
    assert!(!rotated_text.contains(first_key));
    assert!(!rotated_text.contains(second_key));
    assert!(!Arc::ptr_eq(
        &source_runtime,
        &server.state.runtime().unwrap().unwrap()
    ));
    assert_eq!(
        client
            .post(format!("{}/sources/{source_id}/test", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        observed.lock().unwrap().last().map(String::as_str),
        Some("Bearer synthetic-source-key-v2")
    );

    // Route through the source whose key changed. Smart can otherwise select
    // the equally healthy backup regardless of the legacy source priorities.
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth(management_key)
            .json(&json!({"sourceIds":[second_source_id],"inPool":false}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{}/gateway/start", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let profile: Value = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let routed_before = observed.lock().unwrap().len();
    assert_eq!(
        client
            .post(format!("{}/v1/responses", server.origin))
            .bearer_auth(profile["secret"].as_str().unwrap())
            .json(&json!({"model":"gpt-source-lifecycle","input":"route"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    {
        let observed = observed.lock().unwrap();
        assert!(observed.len() > routed_before);
        assert_eq!(
            observed.last().map(String::as_str),
            Some("Bearer synthetic-source-key-v2")
        );
    }

    let snapshot_text = client
        .get(format!("{}/state", server.origin))
        .bearer_auth(management_key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!snapshot_text.contains(first_key));
    assert!(!snapshot_text.contains(second_key));
    assert_eq!(
        client
            .delete(format!("{}/sources/{source_id}", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(server
        .state
        .vault
        .load(&format!("source:{source_id}"))
        .unwrap()
        .is_none());
    assert_eq!(
        client
            .delete(format!("{}/sources/{second_source_id}", server.origin))
            .bearer_auth(management_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(server.state.snapshot().unwrap().sources.is_empty());

    server.task.abort();
    upstream_task.abort();
}

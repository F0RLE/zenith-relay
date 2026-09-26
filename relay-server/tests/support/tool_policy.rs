use super::*;
use zenith_relay_core::{ToolPolicy, ToolPolicyMode, ToolPolicyUpdate};

#[tokio::test]
async fn preset_versions_preserve_omitted_tool_policy_and_allow_explicit_reset() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();
    let policy = ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    };
    client
        .post(format!("{}/gateway/tool-policy", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&ToolPolicyUpdate {
            policy: policy.clone(),
            expected_policy: ToolPolicy::default(),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let document: Value = client
        .get(format!("{}/configuration/preset", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut preset = document["preset"].clone();
    assert_eq!(preset["schemaVersion"], 6);
    assert_eq!(preset["settings"]["routing"]["toolPolicy"], json!(policy));
    preset["settings"]["routing"]
        .as_object_mut()
        .unwrap()
        .remove("toolPolicy");

    for version in 2..=6 {
        preset["schemaVersion"] = json!(version);
        apply_tool_policy_preset(&client, &server, &preset).await;
        assert_eq!(server.state.snapshot().unwrap().gateway.tool_policy, policy);
    }
    preset["settings"]["routing"]["toolPolicy"] = Value::Null;
    apply_tool_policy_preset(&client, &server, &preset).await;
    assert_eq!(server.state.snapshot().unwrap().gateway.tool_policy, policy);

    for version in [0, 1, 7, u16::MAX] {
        preset["schemaVersion"] = json!(version);
        let rejected = client
            .post(format!("{}/configuration/preset/preview", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({"preset": preset}))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_eq!(server.state.snapshot().unwrap().gateway.tool_policy, policy);
    }

    preset["schemaVersion"] = json!(5);
    preset["settings"]["routing"]["toolPolicy"] = json!({"mode": "pass_through"});
    apply_tool_policy_preset(&client, &server, &preset).await;
    assert_eq!(
        server.state.snapshot().unwrap().gateway.tool_policy,
        ToolPolicy::default()
    );
    assert_eq!(
        server.state.store.routing_policy().unwrap().tool_policy,
        Some(ToolPolicy::default())
    );
    server.task.abort();
}

async fn apply_tool_policy_preset(
    client: &reqwest::Client,
    server: &RunningServer,
    preset: &Value,
) {
    let preview: Value = client
        .post(format!("{}/configuration/preset/preview", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"preset": preset}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let applied = client
        .post(format!("{}/configuration/preset/apply", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "baseRevision": preview["baseRevision"],
            "preset": preview["preset"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(applied.status(), StatusCode::OK);
}

#[tokio::test]
async fn tool_policy_management_is_authenticated_atomic_hot_and_restart_safe() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    add_rebuild_failing_source(&server.state);
    let mut source = server.state.store.sources().unwrap().remove(0);
    source.weight = 1;
    server.state.store.save_source(&source).unwrap();
    server.state.rebuild_runtime().await.unwrap();
    let runtime = server.state.runtime().unwrap().unwrap();
    let client = reqwest::Client::new();
    let url = format!("{}/gateway/tool-policy", server.origin);
    let update = ToolPolicyUpdate {
        policy: ToolPolicy {
            mode: ToolPolicyMode::Automatic,
        },
        expected_policy: ToolPolicy::default(),
    };
    assert_eq!(
        client
            .post(&url)
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let save = client
        .post(&url)
        .bearer_auth("synthetic-management-token-value")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(save.status(), StatusCode::OK);
    let snapshot: Value = save.json().await.unwrap();
    assert_eq!(
        snapshot["gateway"]["toolPolicy"]["mode"],
        json!("automatic")
    );
    assert!(snapshot["gateway"]["toolPolicy"]
        .as_object()
        .unwrap()
        .get("automaticToolCountThreshold")
        .is_none());
    assert!(snapshot["gateway"]["toolPolicy"]
        .as_object()
        .unwrap()
        .get("automaticSchemaBytesThreshold")
        .is_none());
    assert!(snapshot["gateway"]["toolPolicy"]
        .as_object()
        .unwrap()
        .get("enabledTools")
        .is_none());
    assert!(snapshot["gateway"]["toolPolicy"]
        .as_object()
        .unwrap()
        .get("disabledTools")
        .is_none());
    assert!(snapshot["capabilities"]["features"]
        .as_array()
        .unwrap()
        .contains(&json!("tool_policy_v1")));
    let normalized = update.policy.clone().normalized().unwrap();
    assert_eq!(runtime.tool_policy(), normalized);
    assert!(Arc::ptr_eq(
        &runtime,
        &server.state.runtime().unwrap().unwrap()
    ));
    let stale = client
        .post(&url)
        .bearer_auth("synthetic-management-token-value")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    // Another routing mutation must not reset the independent tool policy.
    assert_eq!(
        client
            .post(format!("{}/routing/settings", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({"maxRetryCandidates":5}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        server.state.store.routing_policy().unwrap().tool_policy,
        Some(normalized.clone())
    );
    assert_eq!(runtime.tool_policy(), normalized);
    server.task.abort();
    drop(server);
    let reopened = spawn_server(root.path()).await;
    assert_eq!(
        reopened.state.snapshot().unwrap().gateway.tool_policy,
        normalized
    );
    assert_eq!(
        reopened.state.runtime().unwrap().unwrap().tool_policy(),
        normalized
    );
    reopened.task.abort();
}

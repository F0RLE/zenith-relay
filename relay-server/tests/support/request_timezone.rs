use super::*;
use zenith_relay_core::{RequestTimezonePolicy, RequestTimezoneUpdate};

#[tokio::test]
async fn request_timezone_management_is_authenticated_atomic_and_restart_safe() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    add_rebuild_failing_source(&server.state);
    let mut source = server.state.store.sources().unwrap().remove(0);
    source.weight = 1;
    server.state.store.save_source(&source).unwrap();
    server.state.rebuild_runtime().await.unwrap();
    let runtime = server.state.runtime().unwrap().unwrap();
    let client = reqwest::Client::new();
    let url = format!("{}/gateway/request-timezone", server.origin);
    let update = RequestTimezoneUpdate {
        policy: RequestTimezonePolicy {
            enabled: true,
            account_timezones: BTreeMap::from([(
                "synthetic-account".into(),
                "Europe/Minsk".into(),
            )]),
        },
        expected_policy: RequestTimezonePolicy::default(),
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
    let catalog_url = format!("{url}/catalog");
    assert_eq!(
        client.get(&catalog_url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let catalog: Vec<String> = client
        .get(&catalog_url)
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(catalog.contains(&"Europe/Minsk".into()));
    let save = client
        .post(&url)
        .bearer_auth("synthetic-management-token-value")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(save.status(), StatusCode::OK);
    let snapshot: Value = save.json().await.unwrap();
    assert_eq!(snapshot["gateway"]["requestTimezone"], json!(update.policy));
    assert!(snapshot["capabilities"]["features"]
        .as_array()
        .unwrap()
        .contains(&json!("request_timezone_v1")));
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
    let invalid = json!({
        "policy": {"enabled": true, "accountTimezones": {"synthetic-account": "Invalid/Zone"}},
        "expectedPolicy": update.policy
    });
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("synthetic-management-token-value")
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        server.state.store.request_timezone().unwrap(),
        update.policy
    );

    let exported: Value = client
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
    // Portable presets have no host-specific account identifiers.
    assert!(!exported.to_string().contains("requestTimezone"));
    let mut preset = exported["preset"].clone();
    preset["settings"]["routing"]["maxRetryCandidates"] = json!(5);
    super::tool_policy::apply_preset(&client, &server, &preset).await;
    assert_eq!(
        server.state.store.request_timezone().unwrap(),
        update.policy
    );
    server.task.abort();
    drop(server);

    let reopened = spawn_server(root.path()).await;
    assert_eq!(
        reopened.state.snapshot().unwrap().gateway.request_timezone,
        update.policy
    );
    assert!(reopened.state.runtime().unwrap().is_some());
    reopened.task.abort();
}

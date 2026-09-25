//! Automatic startup conversion, with no user-facing migration operation.
use super::*;
use zenith_relay_core::{PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy};

#[tokio::test]
async fn startup_upgrades_legacy_settings_without_stopping_the_pool_or_changing_permissions() {
    for explicit in [false, true] {
        let root = TempDir::new().unwrap();
        let server = spawn_server(root.path()).await;
        add_rebuild_failing_source(&server.state);
        let mut source = server.state.store.sources().unwrap().remove(0);
        source.weight = 9;
        source.priority = -1_000_000;
        source.recovery_delay_seconds = 45;
        server.state.store.save_source(&source).unwrap();
        let mut routing = server.state.store.routing_policy().unwrap();
        routing.pool_routing = explicit.then(|| PoolRoutingPolicy {
            version: 1,
            mode: PoolRoutingMode::Smart,
            members: vec![PoolRoutingMember {
                kind: PoolMemberKind::Source,
                id: source.id.clone(),
                weight: 7,
                max_concurrency: 2,
            }],
        });
        routing.max_retry_candidates = 8;
        server.state.store.set_routing_policy(&routing).unwrap();
        server.state.store.set_gateway_enabled(true).unwrap();
        server
            .state
            .store
            .set_chatgpt_retry_until_available(true)
            .unwrap();
        server.task.abort();
        drop(server);

        let server = spawn_server(root.path()).await;
        let persisted = server.state.store.routing_policy().unwrap();
        let policy = persisted.pool_routing.as_ref().unwrap();
        assert_eq!(policy.mode, PoolRoutingMode::Automatic);
        assert!(policy.is_current_rotation());
        assert_eq!(policy.members[0].weight, if explicit { 7 } else { 9 });
        assert_eq!(
            policy.members[0].max_concurrency,
            if explicit { 2 } else { 0 }
        );
        assert_eq!(persisted.max_retry_candidates, 8);
        assert!(server.state.store.gateway_enabled().unwrap());
        assert!(server.state.store.chatgpt_retry_until_available().unwrap());
        assert!(server.state.runtime().unwrap().is_some());
        assert_eq!(
            serde_json::to_value(server.state.store.sources().unwrap().remove(0)).unwrap(),
            serde_json::to_value(&source).unwrap()
        );
        assert!(!server
            .state
            .snapshot()
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.contains("migration")));
        let reopened = Store::open(root.path().join("relay.sqlite")).unwrap();
        assert_eq!(reopened.routing_policy().unwrap(), persisted);
        drop(reopened);
        let client = reqwest::Client::new();
        let current = server
            .state
            .snapshot()
            .unwrap()
            .gateway
            .pool_routing
            .unwrap();
        let mut next = current.clone();
        next.mode = PoolRoutingMode::InOrder;
        let updated: serde_json::Value = client
            .post(format!("{}/routing/settings", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({
                "poolRouting": next,
                "expectedPoolRouting": current,
                "maxRetryCandidates": 6,
                "cooldownAfterFailures": 1,
                "keepLastCandidateAvailable": true,
                "routingStrategy": "quota_highest",
                "subscriptionPlanOrder": ["not a valid\nplan"]
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(updated["gateway"]["poolRouting"], json!(next));
        assert_eq!(updated["gateway"]["maxRetryCandidates"], 6);
        let hot = server.state.store.routing_policy().unwrap();
        assert_eq!(hot.pool_routing, Some(next));
        assert_eq!(hot.max_retry_candidates, 6);
        let hot_snapshot = serde_json::to_value(server.state.snapshot().unwrap().gateway).unwrap();
        for old in [
            "cooldownAfterFailures",
            "keepLastCandidateAvailable",
            "routingStrategy",
            "subscriptionPlanOrder",
        ] {
            assert!(hot_snapshot.get(old).is_none());
        }
        assert!(server.state.runtime().unwrap().is_some());
        for path in ["/routing/migration/preview", "/routing/migration/apply"] {
            assert_eq!(
                client
                    .post(format!("{}{path}", server.origin))
                    .bearer_auth("synthetic-management-token-value")
                    .json(&json!({"confirmed":true,"direction":"restore_v1"}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        server.task.abort();
    }
}

#[test]
fn automatic_upgrade_preserves_disabled_gateway_and_is_idempotent() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    let mut routing = store.routing_policy().unwrap();
    routing.pool_routing = None;
    store.set_routing_policy(&routing).unwrap();
    store.set_gateway_enabled(false).unwrap();
    drop(store);
    for _ in 0..2 {
        let store = Store::open(path.clone()).unwrap();
        assert!(store
            .routing_policy()
            .unwrap()
            .pool_routing
            .unwrap()
            .is_current_rotation());
        assert!(!store.gateway_enabled().unwrap());
    }
}

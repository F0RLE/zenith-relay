use super::*;
use zenith_relay_core::{PoolRoutingMode, PoolRoutingPolicy};

fn legacy_policy() -> PoolRoutingPolicy {
    PoolRoutingPolicy {
        version: 1,
        mode: PoolRoutingMode::Smart,
        members: Vec::new(),
    }
}

#[test]
fn startup_upgrades_old_gateway_settings_automatically_and_preserves_controls() {
    for enabled in [true, false] {
        for explicit in [true, false] {
            let root =
                std::env::temp_dir().join(format!("rotation-upgrade-{}", uuid::Uuid::new_v4()));
            let mut store = LocalPoolStore::open(root.clone()).unwrap();
            let mut gateway = store.gateway().clone();
            gateway.enabled = enabled;
            gateway.pool_routing = explicit.then(legacy_policy);
            gateway.max_retry_candidates = 8;
            gateway.chatgpt_retry_until_available = true;
            store.replace_gateway(gateway).unwrap();
            let mut old = serde_json::to_value(store.gateway()).unwrap();
            old["cooldownAfterFailures"] = serde_json::json!(0);
            old["keepLastCandidateAvailable"] = serde_json::json!(false);
            old["routingStrategy"] = serde_json::json!("quota_highest");
            old["subscriptionPlanOrder"] = serde_json::json!(["not a valid\nplan"]);
            store
                .database
                .replace_state_json(&[(STATE_GATEWAY, old.to_string())])
                .unwrap();
            drop(store);
            for _ in 0..2 {
                let reopened = LocalPoolStore::open(root.clone()).unwrap();
                let current = reopened.gateway();
                assert_eq!(current.enabled, enabled);
                assert_eq!(current.max_retry_candidates, 8);
                assert!(current.chatgpt_retry_until_available);
                assert_eq!(
                    current.pool_routing.as_ref().unwrap().mode,
                    PoolRoutingMode::Automatic
                );
                assert!(current.pool_routing.as_ref().unwrap().is_current_rotation());
                let saved: serde_json::Value = serde_json::from_str(
                    &reopened
                        .database
                        .state_json(STATE_GATEWAY)
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
                for old in [
                    "cooldownAfterFailures",
                    "keepLastCandidateAvailable",
                    "routingStrategy",
                    "subscriptionPlanOrder",
                ] {
                    assert!(saved.get(old).is_none(), "obsolete {old} was reserialized");
                }
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn startup_cleans_old_scalars_from_saved_rotation_policy_without_changing_controls() {
    for enabled in [true, false] {
        let root =
            std::env::temp_dir().join(format!("rotation-scalar-cleanup-{}", uuid::Uuid::new_v4()));
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let mut gateway = store.gateway().clone();
        gateway.enabled = enabled;
        gateway.max_retry_candidates = 6;
        gateway.chatgpt_retry_until_available = true;
        gateway.pool_routing.as_mut().unwrap().mode = PoolRoutingMode::RoundRobin;
        store.replace_gateway(gateway.clone()).unwrap();

        let mut saved = serde_json::to_value(&gateway).unwrap();
        saved["cooldownAfterFailures"] = serde_json::json!(0);
        saved["keepLastCandidateAvailable"] = serde_json::json!(false);
        saved["routingStrategy"] = serde_json::json!("quota_highest");
        saved["subscriptionPlanOrder"] = serde_json::json!(["invalid\nplan"]);
        store
            .database
            .replace_state_json(&[(STATE_GATEWAY, saved.to_string())])
            .unwrap();
        drop(store);

        for _ in 0..2 {
            let reopened = LocalPoolStore::open(root.clone()).unwrap();
            assert_eq!(reopened.gateway(), &gateway);
            assert_eq!(
                reopened.gateway().pool_routing.as_ref().unwrap().mode,
                PoolRoutingMode::RoundRobin
            );
            let saved: serde_json::Value = serde_json::from_str(
                &reopened
                    .database
                    .state_json(STATE_GATEWAY)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            for old in [
                "cooldownAfterFailures",
                "keepLastCandidateAvailable",
                "routingStrategy",
                "subscriptionPlanOrder",
            ] {
                assert!(saved.get(old).is_none(), "obsolete {old} was reserialized");
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn old_json_without_policy_is_accepted_and_resolved_to_current_defaults() {
    let mut document = serde_json::to_value(GatewaySettings::default()).unwrap();
    document.as_object_mut().unwrap().remove("poolRouting");
    document["cooldownAfterFailures"] = serde_json::json!(7);
    document["keepLastCandidateAvailable"] = serde_json::json!(false);
    let gateway: GatewaySettings = serde_json::from_value(document).unwrap();
    assert!(gateway.pool_routing.is_none());
    assert!(gateway.pool_routing_for(&[], &[]).is_current_rotation());
}

use super::*;

#[test]
fn runtime_snapshot_keeps_the_management_wire_shape() {
    let snapshot = CandidateRuntimeSnapshot {
        candidate_id: "source".into(),
        kind: CandidateKind::ApiSource,
        available: true,
        next_for_new_request: false,
        activity_revision: 0,
        runtime_id: 0,
        in_flight: 0,
        active_request_count: 0,
        active_models: Vec::new(),
        model_retries: Vec::new(),
        last_used_at_ms: None,
        next_retry_at_ms: None,
        half_open: false,
        dispatches: 0,
    };

    let value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(value["activeRequestCount"], 0);
    assert_eq!(value["activeModels"], serde_json::json!([]));
    assert_eq!(value["modelRetries"], serde_json::json!([]));
    assert!(value.get("active_request_count").is_none());
}
#[test]
fn runtime_order_uses_scheduler_preference_and_exposes_live_state() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("first"));
    scheduler.upsert(candidate("second"));

    let initial = scheduler.runtime_order(50);
    assert_eq!(initial[0].candidate_id, "first");
    assert!(initial.iter().all(|candidate| candidate.available));

    assert!(scheduler.reserve_for("first", "gpt-5", 50));
    let loaded = scheduler.runtime_order(50);
    assert_eq!(loaded[0].candidate_id, "first");
    assert_eq!(loaded[0].in_flight, 1);
    assert_eq!(loaded[0].active_request_count, 1);
    assert_eq!(
        loaded[0].active_models,
        vec![ActiveModelRuntime {
            model: "gpt-5".into(),
            request_count: 1,
        }]
    );
    assert_eq!(loaded[0].dispatches, 0);
    assert_eq!(loaded[0].last_used_at_ms, None);
    assert_eq!(
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 50,
            })
            .unwrap()
            .candidate_id,
        "second"
    );
    assert!(scheduler.record_success("first", "gpt-5", 75));
    assert_eq!(scheduler.runtime_order(75)[0].last_used_at_ms, Some(75));

    assert!(scheduler.set_cooldown("second", "gpt-5", 100));
    let cooling = scheduler.runtime_order(50);
    let second = cooling
        .iter()
        .find(|candidate| candidate.candidate_id == "second")
        .unwrap();
    assert!(!second.available);
    assert_eq!(second.next_retry_at_ms, Some(100));
    assert_eq!(
        second.model_retries,
        vec![ModelRetryRuntime {
            model: "gpt-5".into(),
            retry_at_ms: 100,
        }]
    );

    assert!(scheduler.reserve_for("second", "gpt-5", 101));
    let probing = scheduler.runtime_order(101);
    let second = probing
        .iter()
        .find(|candidate| candidate.candidate_id == "second")
        .unwrap();
    assert!(!second.half_open);
    assert!(second.available);
}
#[test]
fn runtime_order_groups_parallel_requests_by_active_model() {
    let mut scheduler = PoolScheduler::new();
    let mut first = candidate("first");
    first
        .models
        .extend(["claude-opus-5".to_string(), "gpt-image-2".to_string()]);
    scheduler.upsert(first);

    assert!(scheduler.reserve_for("first", "gpt-5", 50));
    assert!(scheduler.reserve_for("first", "gpt-5", 50));
    assert!(scheduler.reserve_for("first", "claude-opus-5", 50));
    assert!(scheduler.reserve_image_for("first", "gpt-image-2", 50));

    let snapshot = scheduler.runtime_order(50).remove(0);
    assert_eq!(snapshot.in_flight, 3);
    assert_eq!(snapshot.active_request_count, 4);
    assert_eq!(
        snapshot.active_models,
        vec![
            ActiveModelRuntime {
                model: "claude-opus-5".into(),
                request_count: 1,
            },
            ActiveModelRuntime {
                model: "gpt-5".into(),
                request_count: 2,
            },
            ActiveModelRuntime {
                model: "gpt-image-2".into(),
                request_count: 1,
            },
        ]
    );

    assert!(scheduler.release_for("first", Some("gpt-5")));
    assert!(scheduler.release_for("first", Some("gpt-5")));
    assert!(scheduler.release_for("first", Some("claude-opus-5")));
    assert!(scheduler.release_image_for("first", Some("gpt-image-2")));
    let released = scheduler.runtime_order(50).remove(0);
    assert_eq!(released.active_request_count, 0);
    assert!(released.active_models.is_empty());
}

use super::*;

fn aliased_owner() -> PoolScheduler {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("a-cold"));
    scheduler.upsert(candidate("z-warm"));
    let mut alias = candidate("z-warm::chat");
    alias.source_id = "z-warm".into();
    alias.protocol = WireApi::ChatCompletions;
    scheduler.upsert(alias);
    scheduler.set_native_route("z-warm", true);
    assert!(scheduler.bind_response_affinity("response", "z-warm::chat", 0));
    scheduler
}

fn should_yield(scheduler: &mut PoolScheduler, tried: &HashSet<String>) -> bool {
    scheduler.automatic_response_owner_should_yield(
        "response",
        "gpt-5",
        &[WireApi::Responses, WireApi::ChatCompletions],
        &CandidateScope::default(),
        tried,
        100,
    )
}

#[test]
fn response_owner_probe_preserves_the_warm_member_after_alias_deduplication() {
    let mut scheduler = aliased_owner();
    assert!(!should_yield(&mut scheduler, &HashSet::new()));
    // The soft admission probe must not rewrite opaque response ownership.
    assert_eq!(
        scheduler
            .response_affinity_candidate("response", 100)
            .as_deref(),
        Some("z-warm::chat")
    );
    let exact = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .unwrap();
    assert_eq!(exact.candidate_id, "z-warm::chat");
}

#[test]
fn response_owner_probe_still_yields_when_the_member_is_excluded() {
    let mut scheduler = aliased_owner();
    assert!(should_yield(
        &mut scheduler,
        &HashSet::from(["z-warm".into(), "z-warm::chat".into()]),
    ));
}

#[test]
fn response_owner_probe_uses_warm_quota_protection_without_moving_opaque_ownership() {
    for (owner, other, yields) in [
        (3_000, 3_500, false),
        (3_000, 4_000, false),
        (3_000, 9_000, false),
        (1_001, 10_000, false),
        (1_000, 1_999, false),
        (1_000, 2_000, true),
    ] {
        let mut scheduler = aliased_owner();
        for (id, remaining) in [
            ("a-cold", other),
            ("z-warm", owner),
            ("z-warm::chat", owner),
        ] {
            let mut member = scheduler.candidate(id).unwrap().clone();
            member.quota = CandidateQuota::Available(remaining);
            member.quota_updated_at_ms = Some(100);
            scheduler.upsert(member);
        }
        assert_eq!(should_yield(&mut scheduler, &HashSet::new()), yields);
        let exact = scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: Some("response"),
                prompt_affinity_key: None,
                now_ms: 100,
            })
            .unwrap();
        assert_eq!(exact.candidate_id, "z-warm::chat");
    }
}

#[test]
fn cache_affinity_diagnostics_follow_the_projected_owner_and_keep_legacy_rows_readable() {
    let mut scheduler = aliased_owner();
    assert!(scheduler.bind_prompt_affinity("cache:private", "z-warm::chat", 0));
    let selected = scheduler
        .select_with_rotation_capture(
            SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: Some("cache:private"),
                now_ms: 100,
            },
            RotationOperation::Text,
            None,
        )
        .unwrap();
    let diagnostics = selected.diagnostics.cache_affinity.as_ref().unwrap();
    assert_eq!(
        diagnostics.reason,
        crate::scheduler::rotation::CacheAffinityReason::Retained
    );
    let mut encoded = serde_json::to_value(&selected.diagnostics).unwrap();
    assert!(!encoded.to_string().contains("cache:private"));
    let latency = selected.diagnostics.route_latency.as_ref().unwrap();
    assert_eq!(latency.as_of_ms, 100);
    assert_eq!(latency.sample_count, 0);
    assert_eq!(latency.unknown_count, 0);
    assert_eq!(encoded["routeLatency"]["state"], "unknown");
    encoded.as_object_mut().unwrap().remove("cacheAffinity");
    encoded.as_object_mut().unwrap().remove("routeLatency");
    let legacy: crate::RoutingDiagnostics = serde_json::from_value(encoded).unwrap();
    assert!(legacy.cache_affinity.is_none());
    assert!(legacy.route_latency.is_none());
}

#[test]
fn cache_affinity_diagnostics_keep_owner_evidence_after_structural_exclusion() {
    let mut scheduler = aliased_owner();
    assert!(scheduler.bind_prompt_affinity("cache:private", "z-warm::chat", 0));
    let selected = scheduler
        .select_with_rotation_capture(
            SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
                scope: &CandidateScope::default(),
                tried: &HashSet::from(["z-warm".into(), "z-warm::chat".into()]),
                response_affinity_key: None,
                prompt_affinity_key: Some("cache:private"),
                now_ms: 100,
            },
            RotationOperation::Text,
            None,
        )
        .unwrap();
    assert_eq!(selected.candidate_id, "a-cold");
    let diagnostics = selected.diagnostics.cache_affinity.unwrap();
    assert_eq!(
        diagnostics.reason,
        crate::scheduler::rotation::CacheAffinityReason::OwnerUnavailable
    );
    assert_eq!(
        diagnostics.owner_block_reason,
        Some(crate::scheduler::rotation::CandidateBlockReason::AlreadyTried)
    );
    assert!(diagnostics.preferred.is_some());
}

#[test]
fn optional_routing_diagnostics_do_not_change_selection() {
    let mut quiet = aliased_owner();
    assert!(quiet.bind_prompt_affinity("cache:synthetic", "z-warm::chat", 0));
    let mut debug = quiet.clone();
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let request = || SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
        scope: &scope,
        tried: &tried,
        response_affinity_key: None,
        prompt_affinity_key: Some("cache:synthetic"),
        now_ms: 100,
    };
    let quiet = quiet.select(request()).unwrap();
    let mut debug = debug
        .select_with_rotation_capture(request(), RotationOperation::Text, None)
        .unwrap();
    assert!(quiet.diagnostics.cache_affinity.is_none());
    assert!(quiet.diagnostics.route_latency.is_none());
    assert!(debug.diagnostics.cache_affinity.take().is_some());
    assert!(debug.diagnostics.route_latency.take().is_some());
    // Independent admissions allocate distinct request identities.
    assert_ne!(
        quiet.rotation_request.request_id,
        debug.rotation_request.request_id
    );
    debug.rotation_request.request_id = quiet.rotation_request.request_id;
    assert_eq!(quiet, debug);
}

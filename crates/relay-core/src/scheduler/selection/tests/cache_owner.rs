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
fn response_owner_probe_still_yields_to_a_material_quota_lead() {
    let mut scheduler = aliased_owner();
    for (id, remaining) in [
        ("a-cold", 9_000),
        ("z-warm", 4_000),
        ("z-warm::chat", 4_000),
    ] {
        let mut member = scheduler.candidate(id).unwrap().clone();
        member.quota = CandidateQuota::Available(remaining);
        member.quota_updated_at_ms = Some(100);
        scheduler.upsert(member);
    }
    assert!(should_yield(&mut scheduler, &HashSet::new()));
}

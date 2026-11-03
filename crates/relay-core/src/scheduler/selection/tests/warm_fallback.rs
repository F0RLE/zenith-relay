use super::*;

fn busy_owner() -> PoolScheduler {
    let mut scheduler = PoolScheduler::new();
    for id in ["owner", "a-cold", "z-warm"] {
        let mut member = oauth_candidate(id);
        if id == "a-cold" {
            member.weight = 100;
        }
        scheduler.upsert(member);
    }
    assert!(scheduler.bind_prompt_affinity("cache:thread", "owner", 100));
    assert!(scheduler.reserve_for("owner", "gpt-5", 100));
    assert!(scheduler.reserve_for("owner", "gpt-5", 100));
    scheduler
}

#[test]
fn repeated_spillovers_reuse_successful_alternative_without_moving_owner() {
    let mut scheduler = busy_owner();
    // A successful spillover is a soft reuse hint, not a durable owner move.
    assert!(!scheduler.bind_prompt_affinity_sticky("cache:thread", "z-warm", 101));
    for now in 102..114 {
        assert_eq!(
            rotation::dispatch(&mut scheduler, now, Some("cache:thread")),
            "z-warm"
        );
        assert!(!scheduler.bind_prompt_affinity_sticky("cache:thread", "z-warm", now));
        assert_eq!(
            scheduler
                .prompt_affinity_snapshot("cache:thread", now)
                .unwrap()
                .0,
            "owner"
        );
    }
    assert!(scheduler.release_for("owner", Some("gpt-5")));
    assert!(scheduler.release_for("owner", Some("gpt-5")));
    assert_eq!(
        rotation::dispatch(&mut scheduler, 115, Some("cache:thread")),
        "owner"
    );
}

#[test]
fn fallback_hint_stays_inside_its_chat_and_admitted_candidates() {
    for case in ["other-chat", "disabled", "exhausted", "tried", "busy"] {
        let mut scheduler = busy_owner();
        assert!(!scheduler.bind_prompt_affinity_sticky("cache:thread", "z-warm", 101));
        let mut tried = HashSet::new();
        match case {
            "disabled" => scheduler.candidates.get_mut("z-warm").unwrap().enabled = false,
            "exhausted" => {
                scheduler.candidates.get_mut("z-warm").unwrap().quota = CandidateQuota::Exhausted;
            }
            "tried" => {
                tried.insert("z-warm".to_string());
            }
            "busy" => {
                assert!(scheduler.reserve_for("z-warm", "gpt-5", 101));
            }
            _ => {}
        }
        let selected = scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &tried,
                response_affinity_key: None,
                prompt_affinity_key: Some(if case == "other-chat" {
                    "cache:other"
                } else {
                    "cache:thread"
                }),
                now_ms: 102,
            })
            .unwrap();
        assert_eq!(selected.candidate_id, "a-cold", "{case}");
    }
}

#[test]
fn fallback_hint_respects_resource_ranking_and_opaque_owner() {
    let mut scheduler = busy_owner();
    assert!(!scheduler.bind_prompt_affinity_sticky("cache:thread", "z-warm", 101));
    for (id, remaining) in [("owner", 9_000), ("a-cold", 9_000), ("z-warm", 4_000)] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::Available(remaining);
        member.quota_updated_at_ms = Some(101);
    }
    assert_eq!(
        rotation::dispatch(&mut scheduler, 102, Some("cache:thread")),
        "a-cold"
    );
    assert!(scheduler.bind_response_affinity("response", "owner", 102));
    let selected = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: Some("cache:thread"),
            now_ms: 103,
        })
        .unwrap();
    assert_eq!(selected.candidate_id, "owner");
    assert!(selected.response_affinity_hit);
}

#[test]
fn successful_alternative_follows_its_admitted_physical_route() {
    let mut scheduler = busy_owner();
    let mut alias = oauth_candidate("z-warm::chat");
    alias.account_id = Some("z-warm".into());
    alias.source_id = "z-warm".into();
    alias.protocol = WireApi::ChatCompletions;
    scheduler.upsert(alias);
    scheduler.set_native_route("z-warm", true);
    assert!(!scheduler.bind_prompt_affinity_sticky("cache:thread", "z-warm::chat", 101));
    assert_eq!(
        rotation::dispatch(&mut scheduler, 102, Some("cache:thread")),
        "z-warm"
    );
}

use super::*;

#[test]
fn prompt_cache_affinity_wins_over_a_large_quota_difference() {
    let mut scheduler = PoolScheduler::new();
    let mut cached = oauth_candidate("cached");
    cached.quota = CandidateQuota::Available(1_000);
    scheduler.upsert(cached);
    let mut fullest = oauth_candidate("fullest");
    fullest.quota = CandidateQuota::Available(9_000);
    scheduler.upsert(fullest);
    assert!(scheduler.bind_prompt_affinity("cache:thread", "cached", 0));

    let selected = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: Some("cache:thread"),
            now_ms: 1,
        })
        .unwrap();

    assert_eq!(selected.candidate_id, "cached");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::PromptCacheAffinity
    );
}
#[test]
fn sticky_prompt_affinity_does_not_rebind_to_spillover_candidate() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(oauth_candidate("owner"));
    scheduler.upsert(oauth_candidate("spillover"));
    assert!(scheduler.bind_prompt_affinity("session:thread", "owner", 0));

    assert!(!scheduler.bind_prompt_affinity_sticky("session:thread", "spillover", 1));
    let selected = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: Some("session:thread"),
            now_ms: 2,
        })
        .unwrap();
    assert_eq!(selected.candidate_id, "owner");

    scheduler.remove("owner");
    assert!(scheduler.bind_prompt_affinity_sticky("session:thread", "spillover", 3));
    assert_eq!(
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: Some("session:thread"),
                now_ms: 4,
            })
            .unwrap()
            .candidate_id,
        "spillover"
    );
}
#[test]
fn response_affinity_is_mandatory() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("creator"));
    let mut fallback = candidate("fallback");
    fallback.priority = 10;
    scheduler.upsert(fallback);
    assert!(scheduler.bind_response_affinity("response", "creator", 0));

    let scope = CandidateScope::default();
    let empty = HashSet::new();
    let selection = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &empty,
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .unwrap();
    assert_eq!(selection.candidate_id, "creator");
    assert!(selection.response_affinity_hit);
    assert_eq!(
        selection.diagnostics.reason,
        SelectionReason::ResponseAffinity
    );

    scheduler.set_cooldown("creator", "gpt-5", 10);
    assert_eq!(
        scheduler.select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &empty,
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        }),
        None,
        "a continuation cannot move to a candidate that did not create the response"
    );
}
#[test]
fn response_affinity_owner_outside_key_scope_can_be_reset_for_fallback() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    let mut fallback = candidate("fallback");
    fallback.priority = 10;
    scheduler.upsert(fallback);
    assert!(scheduler.bind_response_affinity("response", "owner", 0));

    let scope = CandidateScope {
        source_ids: Some(BTreeSet::from(["fallback".to_string()])),
        ..CandidateScope::default()
    };
    assert_eq!(
        scheduler.response_affinity_owner_supports_route(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(false),
        "a removed pool member must not keep an opaque response pinned forever"
    );

    // Affinity selection remains mandatory until the caller resets the
    // opaque continuation, preserving the safety rule for a still-configured
    // owner while allowing the request layer to make the membership change
    // explicit before choosing the fallback.
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .is_none());
    assert!(scheduler.invalidate_response_affinity("response"));
    assert_eq!(
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &HashSet::new(),
                response_affinity_key: Some("response"),
                prompt_affinity_key: None,
                now_ms: 1,
            })
            .unwrap()
            .candidate_id,
        "fallback"
    );
}
#[test]
fn optional_affinity_reports_a_temporarily_unavailable_owner() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    scheduler.upsert(candidate("fallback"));
    assert!(scheduler.bind_response_affinity("response", "owner", 0));

    let scope = CandidateScope::default();
    assert_eq!(
        scheduler.response_affinity_owner_supports_route(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(true)
    );
    assert_eq!(
        scheduler.response_affinity_owner_is_eligible(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(true)
    );

    assert!(scheduler.set_candidate_health("owner", CandidateHealth::ReauthRequired));
    assert_eq!(
        scheduler.response_affinity_owner_supports_route(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(true),
        "reauth is a temporary availability state, not a route-shape change"
    );
    assert_eq!(
        scheduler.response_affinity_owner_is_eligible(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(false)
    );
}
#[test]
fn removed_candidate_keeps_response_owner_only_until_a_replacement_id_is_upserted() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    scheduler.upsert(candidate("fallback"));
    assert!(scheduler.bind_response_affinity("response", "owner", 0));

    assert!(scheduler.remove("owner").is_some());
    assert_eq!(
        scheduler.response_affinity_candidate("response", 1),
        Some("owner".into())
    );
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .is_none());

    scheduler.upsert(candidate("owner"));
    assert_eq!(scheduler.response_affinity_candidate("response", 1), None);
}
#[test]
fn invalidated_response_affinity_allows_retry_on_another_candidate() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    let mut fallback = candidate("fallback");
    fallback.priority = 10;
    scheduler.upsert(fallback);
    assert!(scheduler.bind_response_affinity("response", "owner", 0));
    scheduler.set_cooldown("owner", "gpt-5", 10);

    // Retryable provider failures invalidate the owner binding before the
    // next selection. The tried set then excludes the cooled owner and lets
    // the scheduler use the next eligible candidate.
    assert!(scheduler.invalidate_response_affinity("response"));
    let tried = HashSet::from(["owner".to_string()]);
    let selection = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &tried,
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .unwrap();
    assert_eq!(selection.candidate_id, "fallback");
    assert!(!selection.response_affinity_hit);
}
#[test]
fn affinity_retry_time_uses_only_the_response_owner() {
    let mut scheduler = PoolScheduler::new();
    let mut owner = candidate("owner");
    owner.cooldowns.insert("gpt-5".into(), 300);
    scheduler.upsert(owner);
    let mut other = candidate("other");
    other.cooldowns.insert("gpt-5".into(), 200);
    scheduler.upsert(other);
    assert!(scheduler.bind_response_affinity("response", "owner", 100));

    let scope = CandidateScope::default();
    assert_eq!(
        scheduler.earliest_retry_at(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 100,
        }),
        Some(300)
    );
}

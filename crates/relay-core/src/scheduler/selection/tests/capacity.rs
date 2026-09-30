use super::*;

#[test]
fn image_lane_is_separate_from_text_load_and_caps_each_oauth_account() {
    let mut first = oauth_candidate("first");
    first.models.insert("gpt-image-2".to_string());
    let mut second = oauth_candidate("second");
    second.models.insert("gpt-image-2".to_string());
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(first);
    scheduler.upsert(second);

    assert!(scheduler.reserve_for("first", "gpt-5", 100));
    let image = select_image(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(image.candidate_id, "second");
    assert_eq!(image.diagnostics.in_flight_before, 0);
    assert!(scheduler.reserve_image_for("first", "gpt-image-2", 100));

    let next_image = select_image(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(next_image.candidate_id, "second");
    let text = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(text.candidate_id, "second");
    assert_eq!(text.diagnostics.in_flight_before, 0);

    assert!(scheduler.release_image_for("first", Some("gpt-image-2")));
    assert!(scheduler.release_for("first", Some("gpt-5")));
}
#[test]
fn oauth_text_leases_allow_parallel_account_requests() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(oauth_candidate("oauth"));
    scheduler.upsert(candidate("api"));

    assert!(scheduler.reserve_for("oauth", "gpt-5", 100));
    assert!(scheduler.reserve_for("oauth", "gpt-5", 100));
    assert!(scheduler.reserve_for("api", "gpt-5", 100));
    assert!(scheduler.reserve_for("api", "gpt-5", 100));

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "api"
    );
    assert!(scheduler.release_for("oauth", Some("gpt-5")));
    assert!(scheduler.release_for("oauth", Some("gpt-5")));
    assert!(scheduler.release_for("api", Some("gpt-5")));
    assert!(scheduler.release_for("api", Some("gpt-5")));
}
#[test]
fn removing_a_busy_candidate_drains_its_lease_before_final_removal() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("busy"));
    assert!(scheduler.reserve_for("busy", "gpt-5", 100));

    assert!(scheduler.remove("busy").is_some());
    // The candidate is no longer selectable, but its activity remains visible
    // so the in-flight request can release its lease normally.
    assert!(scheduler.candidate("busy").is_some());
    assert_eq!(scheduler.runtime_activity_for("busy").1, 1);
    assert!(select(&mut scheduler, &HashSet::new()).is_none());

    assert!(scheduler.release_for("busy", Some("gpt-5")));
    assert!(scheduler.candidate("busy").is_none());
    assert_eq!(scheduler.runtime_activity_for("busy").1, 0);
}
#[test]
fn occupied_oauth_account_remains_eligible_for_text_selection() {
    let mut scheduler = PoolScheduler::new();
    let mut busy = oauth_candidate("busy");
    busy.quota = CandidateQuota::Available(5_000);
    scheduler.upsert(busy);
    let mut free = oauth_candidate("free");
    free.quota = CandidateQuota::Available(4_999);
    scheduler.upsert(free);
    assert!(scheduler.reserve("busy"));

    let selected = select(&mut scheduler, &HashSet::new()).unwrap();

    assert_eq!(selected.candidate_id, "free");
    assert_eq!(selected.diagnostics.reason, SelectionReason::ParallelLoad);
}
#[test]
fn one_oauth_account_accepts_parallel_text_requests() {
    let mut scheduler = PoolScheduler::new();
    let mut account = oauth_candidate("only");
    account.quota = CandidateQuota::Available(5_000);
    scheduler.upsert(account);

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "only"
    );
    assert!(scheduler.reserve("only"));
    let second = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(second.candidate_id, "only");
    assert_eq!(second.diagnostics.in_flight_before, 1);
    assert!(scheduler.reserve("only"));
    assert!(scheduler.release("only"));
    assert!(scheduler.release("only"));
}
#[test]
fn concurrent_requests_fill_each_oauth_account_once() {
    let mut scheduler = PoolScheduler::new();
    for (id, quota) in [
        ("sixty-three", 6_300),
        ("fifty-four", 5_400),
        ("fifty-two", 5_200),
        ("fifty-one", 5_100),
    ] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(quota);
        scheduler.upsert(account);
    }

    let mut counts = BTreeMap::new();
    for _ in 0..200 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert!(scheduler.reserve(&selected.candidate_id));
        *counts.entry(selected.candidate_id).or_insert(0_u32) += 1;
    }

    assert_eq!(
        counts,
        [
            ("sixty-three".into(), 50),
            ("fifty-four".into(), 50),
            ("fifty-two".into(), 50),
            ("fifty-one".into(), 50),
        ]
        .into()
    );
    for (id, count) in counts {
        for _ in 0..count {
            assert!(scheduler.release(&id));
        }
    }
}
#[test]
fn execution_fences_are_reference_counted_and_capability_blocks_are_model_scoped() {
    let mut scheduler = PoolScheduler::new();
    let mut account = oauth_candidate("account");
    account.models.insert("gpt-5-mini".into());
    scheduler.upsert(account);

    assert!(scheduler.set_execution_fence("account", true));
    assert!(scheduler.set_execution_fence("account", true));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert!(scheduler.set_execution_fence("account", false));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert!(scheduler.set_execution_fence("account", false));

    assert!(scheduler.block_capability("account", "gpt-5"));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5-mini",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .is_some());
    assert!(scheduler.clear_capability_blocks("account"));
    assert!(select(&mut scheduler, &HashSet::new()).is_some());
}
#[test]
fn old_dispatch_fence_cannot_release_a_reintroduced_candidate_fence() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("source"));
    let old_epoch = scheduler.begin_execution_fence("source").unwrap();
    scheduler.remove("source").unwrap();
    scheduler.upsert(candidate("source"));
    let new_epoch = scheduler.begin_execution_fence("source").unwrap();
    assert_ne!(old_epoch, new_epoch);

    scheduler.end_execution_fence("source", old_epoch);
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    scheduler.end_execution_fence("source", new_epoch);
    assert!(select(&mut scheduler, &HashSet::new()).is_some());
}

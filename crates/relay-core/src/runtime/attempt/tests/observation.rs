use super::*;

#[tokio::test]
async fn cooldown_is_visible_to_release_observers_and_revokes_pending_dispatch() {
    let runtime = runtime();
    let first_budget = SharedRequestBudget::for_incoming_request(3);
    let pending_budget = SharedRequestBudget::for_incoming_request(3);
    let first = reserve(&runtime, &first_budget, WireApi::Responses).await;
    let pending = reserve(&runtime, &pending_budget, WireApi::Responses).await;
    assert_eq!(first.candidate_id(), pending.candidate_id());
    first.begin_rotation_dispatch().unwrap();
    let deadline = crate::unix_time_ms() + 60_000;
    let seen = Arc::new(AtomicBool::new(false));
    let observed = seen.clone();
    let weak = Arc::downgrade(&runtime);
    runtime.set_activity_callback(move |_| {
        let runtime = weak.upgrade().unwrap();
        let scheduler = runtime.lock_scheduler();
        for candidate in scheduler
            .candidates()
            .filter(|candidate| candidate.source_id == "source-a")
        {
            assert_eq!(
                candidate.cooldowns.get("model-a"),
                Some(&deadline),
                "{}",
                candidate.id
            );
            assert!(!candidate.cooldowns.contains_key("model-b"));
        }
        observed.store(true, Ordering::Release);
    });
    runtime.settle_rotation_failure(
        &first,
        rejected(),
        Some(CooldownRequest {
            scope: "model-a",
            retry_at_ms: deadline,
            reason: CooldownReason::RateLimit,
        }),
        crate::unix_time_ms(),
    );
    assert!(
        seen.load(Ordering::Acquire),
        "release callback must see the installed block"
    );
    assert_eq!(
        pending.begin_rotation_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(
        pending_budget.dispatches(),
        0,
        "lost dispatch race is not a generation"
    );
    assert!(pending_budget.attempted_members().is_empty());
    // A duplicate outcome cannot extend the block or vote again.
    runtime.settle_rotation_failure(
        &first,
        rejected(),
        Some(CooldownRequest {
            scope: "model-a",
            retry_at_ms: deadline + 60_000,
            reason: CooldownReason::RateLimit,
        }),
        crate::unix_time_ms(),
    );
    assert_eq!(
        runtime
            .failure_state_for(first.candidate_id(), "model-a", crate::unix_time_ms())
            .1,
        Some(("model-a".into(), deadline))
    );
    drop(pending);
    let alternative = reserve(&runtime, &first_budget, WireApi::Responses).await;
    assert_eq!(alternative.member_key, "source:source-b");
}

#[tokio::test]
async fn a_new_driver_keeps_physical_attempt_history_but_explicit_repair_can_retry_owner() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let first = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(first.member_key, "source:source-a");
    first.begin_rotation_dispatch().unwrap();
    first
        .settle_rotation(rejected(), crate::unix_time_ms())
        .unwrap();
    let handoff = budget.clone();
    let next = reserve(&runtime, &handoff, WireApi::ChatCompletions).await;
    assert_eq!(
        next.member_key, "source:source-b",
        "an alias of A is not an untried source"
    );
    drop(next);
    first.allow_rotation_repair();
    assert_eq!(handoff.dispatches(), 1, "repair never refunds a dispatch");
    let repair = reserve(&runtime, &handoff, WireApi::Responses).await;
    assert_eq!(repair.member_key, "source:source-a");
    repair.begin_rotation_dispatch().unwrap();
    repair.settle_rotation_unknown(crate::unix_time_ms());
    repair.allow_rotation_repair();
    handoff.begin_recovery_pass();
    assert!(
        !handoff.can_dispatch(),
        "neither repair nor recovery resets unknown execution"
    );
    assert_eq!(handoff.dispatches(), 2);
}

#[tokio::test]
async fn local_route_incompatibility_does_not_visit_or_exclude_its_member() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let first = reserve(&runtime, &budget, WireApi::Responses).await;
    let incompatible = HashSet::from([first.candidate_id().to_owned()]);
    drop(first);
    let (selection, alias) = runtime
        .select_and_reserve_with_budget(
            &key(&runtime),
            "model-a",
            &[WireApi::Responses, WireApi::ChatCompletions],
            &incompatible,
            (None, None),
            crate::unix_time_ms(),
            &budget,
        )
        .await
        .unwrap();
    assert!(!incompatible.contains(&selection.candidate_id));
    assert_eq!(alias.member_key, "source:source-a");
    assert_eq!(budget.dispatches(), 0);
    assert!(budget.attempted_members().is_empty());
}

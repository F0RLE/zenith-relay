use super::*;

#[test]
fn ambiguous_routes_use_expiry_display_priority_without_fabricating_dispatch_order() {
    let mut scheduler = PoolScheduler::new();
    let now = 100;
    for (id, quota) in [("ordinary", 10_000), ("urgent", 1_000)] {
        let mut member = oauth_candidate(id);
        member.models = BTreeSet::from([format!("model-{id}")]);
        member.quota = CandidateQuota::Available(quota);
        member.quota_updated_at_ms = Some(now);
        scheduler.upsert(member);
    }
    let deadline = crate::quota::QuotaSpendDeadline {
        expires_at_ms: now + 2 * 86_400_000,
        observed_at_ms: now,
        valid_until_ms: now + 1_000,
        reset_credits: None,
    };
    scheduler.set_spend_deadlines(BTreeMap::from([("urgent".into(), deadline)]));
    for mode in [
        crate::PoolRoutingMode::Automatic,
        crate::PoolRoutingMode::SubscriptionExpiry,
        crate::PoolRoutingMode::RoundRobin,
        crate::PoolRoutingMode::WeightedRoundRobin,
        crate::PoolRoutingMode::Failover,
    ] {
        let mut policy = scheduler.migrated_pool_routing();
        policy.mode = mode;
        scheduler.set_pool_routing(policy).unwrap();
        let order = scheduler.runtime_order(now);
        assert_eq!(
            order[0].candidate_id,
            if mode.is_adaptive() {
                "urgent"
            } else {
                "ordinary"
            }
        );
        assert!(order.iter().all(|member| member.selection_rank.is_none()
            && !member.next_for_new_request
            && member.dispatches == 0));
        assert_eq!(
            scheduler.runtime_order(now),
            order,
            "preview must not mutate the live rotation"
        );
        assert_eq!(
            scheduler.runtime_order(now + 1_001)[0].candidate_id,
            "ordinary",
            "stale entitlement must lose priority"
        );
    }
}

#[test]
fn subscription_expiry_display_orders_exact_dates_within_one_band() {
    let mut scheduler = PoolScheduler::new();
    let mut deadlines = BTreeMap::new();
    for (id, days, quota) in [("later", 2, 10_000), ("earlier", 1, 2_000)] {
        let mut member = oauth_candidate(id);
        member.models = BTreeSet::from([format!("model-{id}")]);
        member.quota = CandidateQuota::Available(quota);
        member.quota_updated_at_ms = Some(100);
        scheduler.upsert(member);
        deadlines.insert(
            id.into(),
            crate::quota::QuotaSpendDeadline {
                expires_at_ms: 100 + days * 86_400_000,
                observed_at_ms: 100,
                valid_until_ms: 1_000,
                reset_credits: None,
            },
        );
    }
    scheduler.set_spend_deadlines(deadlines);
    let mut policy = scheduler.migrated_pool_routing();
    policy.mode = crate::PoolRoutingMode::SubscriptionExpiry;
    scheduler.set_pool_routing(policy).unwrap();
    let order = scheduler.runtime_order(100);
    assert_eq!(order[0].candidate_id, "earlier");
    assert!(order.iter().all(|member| member.selection_rank.is_none()));
    assert_eq!(scheduler.runtime_order(100), order);
}

#[test]
fn runtime_display_groups_nonurgent_dates_independently_of_reset_budgets() {
    let mut scheduler = PoolScheduler::new();
    for (id, quota) in [("a", 10_000), ("b", 1_100)] {
        let mut member = oauth_candidate(id);
        member.models = BTreeSet::from([format!("model-{id}")]);
        member.quota = CandidateQuota::Available(quota);
        member.quota_updated_at_ms = Some(100);
        scheduler.upsert(member);
    }
    let mut deadline = crate::quota::QuotaSpendDeadline {
        expires_at_ms: 100 + 9 * 86_400_000,
        observed_at_ms: 100,
        valid_until_ms: 1_000,
        reset_credits: None,
    };
    let later = crate::quota::QuotaSpendDeadline {
        expires_at_ms: 100 + 17 * 86_400_000,
        ..deadline
    };
    scheduler.set_spend_deadlines(BTreeMap::from([
        ("a".into(), later),
        ("b".into(), deadline),
    ]));
    assert_eq!(scheduler.runtime_order(100)[0].candidate_id, "b");
    assert!(
        scheduler
            .runtime_order(100)
            .iter()
            .all(|member| member.selection_rank.is_none())
    );
    deadline.reset_credits = Some(crate::quota::QuotaSpendResets {
        count: 4,
        observed_at_ms: 100,
        valid_until_ms: 500,
    });
    scheduler.set_spend_deadlines(BTreeMap::from([
        ("a".into(), later),
        ("b".into(), deadline),
    ]));
    assert_eq!(scheduler.runtime_order(100)[0].candidate_id, "b");
    assert_eq!(scheduler.runtime_order(501)[0].candidate_id, "b");
    assert_eq!(scheduler.runtime_order(1_001)[0].candidate_id, "a");
    scheduler
        .candidates
        .get_mut("a")
        .unwrap()
        .quota_updated_at_ms = None;
    assert_eq!(scheduler.runtime_order(1_001)[0].candidate_id, "b");
}

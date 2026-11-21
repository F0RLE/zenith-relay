//! Deterministic policy comparisons over synthetic request and quota evidence.

use super::*;
use crate::quota::{QuotaSpendDeadline, QuotaSpendResets};

const DAY_MS: u64 = 86_400_000;

#[derive(Debug, Default)]
struct ScenarioMetrics {
    served: BTreeMap<String, u64>,
    affinity_hits: u64,
    switches: u64,
}

impl ScenarioMetrics {
    fn complete(
        &mut self,
        engine: &mut RotationEngine,
        request: &RotationRequest,
        now_ms: u64,
    ) -> String {
        let lease = engine.reserve(request, now_ms).unwrap();
        let mut budget = RequestBudget::default_for(request.request_id);
        engine.begin_dispatch(lease.lease_id, &mut budget).unwrap();
        engine
            .settle(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::committed(),
                    health: HealthObservation::Success,
                },
                &budget,
                now_ms,
            )
            .unwrap();
        if let Some(preferred) = &request.preferred {
            if *preferred == lease.candidate_id {
                self.affinity_hits += 1;
            } else {
                self.switches += 1;
            }
        }
        *self.served.entry(lease.candidate_id.clone()).or_default() += 1;
        lease.candidate_id
    }

    fn spend_turn(
        &mut self,
        engine: &mut RotationEngine,
        remaining: &mut BTreeMap<String, u64>,
        id: u64,
    ) {
        let now_ms = 100 + id;
        let selected = self.complete(engine, &request(id), now_ms);
        let quota = remaining.get_mut(&selected).unwrap();
        *quota -= 100;
        engine.set_quota_remaining(&selected, Some(*quota), Some(now_ms));
        if *quota == 0 {
            engine.set_quota(&selected, QuotaState::Exhausted { reset_at_ms: None });
        }
    }
}

fn dated_member(id: &str, days: u64) -> RotationCandidate {
    let mut member = candidate(id, 0, 1);
    member.quota = QuotaState::Available;
    member.quota_remaining_basis_points = Some(10_000);
    member.quota_observed_at_ms = Some(100);
    member.spend_deadline = Some(QuotaSpendDeadline {
        expires_at_ms: 100 + days * DAY_MS,
        observed_at_ms: 100,
        valid_until_ms: DAY_MS,
        reset_credits: None,
    });
    member
}

#[test]
fn compare_mixed_warm_chats_and_independent_work() {
    for mode in [RotationMode::Automatic, RotationMode::RoundRobin] {
        let mut engine = engine_with([dated_member("near", 2), dated_member("warm", 12)]);
        engine.set_mode(mode);
        let mut metrics = ScenarioMetrics::default();
        for id in 1..=200 {
            let request = if id % 2 == 1 {
                preferred_request(id, "warm")
            } else {
                request(id)
            };
            metrics.complete(&mut engine, &request, 100 + id);
        }
        println!("mixed warm/new {mode:?}: {metrics:?}");
        assert_eq!(metrics.served.values().sum::<u64>(), 200);
        if mode == RotationMode::Automatic {
            assert_eq!(metrics.affinity_hits, 100);
            assert_eq!(metrics.switches, 0);
            assert_eq!(metrics.served["near"], 100);
        }
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn warm_quota_run_keeps_cache_until_the_low_water_mark() {
    let mut warm = dated_member("warm", 2);
    warm.quota_remaining_basis_points = Some(6_000);
    let mut engine = engine_with([warm, dated_member("later", 12)]);
    let mut metrics = ScenarioMetrics::default();
    for id in 1..=50 {
        let now_ms = 100 + id;
        assert_eq!(
            metrics.complete(&mut engine, &preferred_request(id, "warm"), now_ms),
            "warm"
        );
        assert!(engine.set_quota_remaining("warm", Some(6_000 - id * 100), Some(now_ms)));
    }
    assert_eq!(metrics.affinity_hits, 50);
    assert_eq!(metrics.switches, 0);
    assert_eq!(
        metrics.complete(&mut engine, &preferred_request(51, "warm"), 151),
        "later"
    );
    assert!(engine.has_material_resource_lead("later", "warm", 151));
    assert_eq!(metrics.switches, 1);
    assert_eq!(engine.active_leases(), 0);
    println!("synthetic warm quota run: {metrics:?}");
}

#[test]
fn compare_quota_left_on_the_earliest_subscription() {
    for mode in [RotationMode::Automatic, RotationMode::RoundRobin] {
        let mut engine = engine_with([dated_member("near", 2), dated_member("later", 12)]);
        engine.set_mode(mode);
        let mut remaining = BTreeMap::from([
            ("near".to_owned(), 10_000_u64),
            ("later".to_owned(), 10_000),
        ]);
        let mut metrics = ScenarioMetrics::default();
        for id in 1..=100 {
            metrics.spend_turn(&mut engine, &mut remaining, id);
        }
        println!("expiry quota {mode:?}: remaining={remaining:?}, {metrics:?}");
        assert_eq!(metrics.served.values().sum::<u64>(), 100);
        assert_eq!(
            remaining["near"],
            if mode == RotationMode::Automatic {
                0
            } else {
                5_000
            }
        );
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn a_single_fast_output_does_not_eliminate_other_measured_members() {
    let mut engine = engine_with([candidate("steady", 0, 1), candidate("fast", 0, 1)]);
    engine.observe_first_output_latency("steady", "responses:gpt", Some(1_000), 100);
    engine.observe_first_output_latency("fast", "responses:gpt", Some(1), 100);
    let mut metrics = ScenarioMetrics::default();
    for id in 1..=20 {
        metrics.complete(&mut engine, &request(id), 100);
    }
    assert_eq!(
        metrics.served.len(),
        2,
        "a single latency sample is not a stable ranking: {metrics:?}"
    );
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn confirmed_reset_budgets_keep_nearest_group_useful() {
    for mode in [RotationMode::Automatic, RotationMode::RoundRobin] {
        let mut near = dated_member("near", 2);
        let mut resets = 2_u32;
        let mut engine = engine_with([near.clone(), dated_member("later", 12)]);
        engine.set_mode(mode);
        let mut remaining = BTreeMap::from([
            ("near".to_owned(), 10_000_u64),
            ("later".to_owned(), 10_000),
        ]);
        let mut metrics = ScenarioMetrics::default();
        for id in 1..=250 {
            let now_ms = 100 + id;
            // Model a confirmed provider ledger, not a redemption decision.
            if remaining["near"] == 0 && resets > 0 {
                resets -= 1;
                remaining.insert("near".into(), 10_000);
                engine.set_quota("near", QuotaState::Available);
                engine.set_quota_remaining("near", Some(10_000), Some(now_ms));
            }
            near.spend_deadline.as_mut().unwrap().reset_credits = Some(QuotaSpendResets {
                count: resets,
                observed_at_ms: now_ms,
                valid_until_ms: DAY_MS,
            });
            engine.upsert(near.clone()).unwrap();
            metrics.spend_turn(&mut engine, &mut remaining, id);
        }
        let expiring_allowance = remaining["near"] + u64::from(resets) * 10_000;
        println!(
            "reset inventory {mode:?}: unused={expiring_allowance}, resets_left={resets}, {metrics:?}"
        );
        assert_eq!(metrics.served.values().sum::<u64>(), 250);
        assert_eq!(
            (expiring_allowance, resets),
            if mode == RotationMode::Automatic {
                (5_000, 0)
            } else {
                (15_000, 1)
            },
        );
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn full_warm_members_release_portable_work_but_keep_opaque_owners() {
    let mut warm = dated_member("warm", 12);
    warm.max_concurrency = 1;
    let mut engine = engine_with([warm, dated_member("near", 2)]);
    let mut metrics = ScenarioMetrics::default();
    for round in 0..20 {
        let id = 4 * round + 1;
        let pending = engine
            .reserve(&request(id).with_owner("warm"), 100)
            .unwrap();
        metrics.complete(&mut engine, &preferred_request(id + 1, "warm"), 100);
        assert!(
            engine
                .reserve(&request(id + 2).with_owner("warm"), 100)
                .is_err()
        );
        assert!(engine.release_unstarted(pending.lease_id));
        metrics.complete(&mut engine, &preferred_request(id + 3, "warm"), 100);
    }
    assert_eq!((metrics.affinity_hits, metrics.switches), (20, 20));
    assert_eq!(metrics.served["near"], 20);
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn provider_wait_overrides_expiry_without_reassigning_a_hard_owner() {
    let mut engine = engine_with([dated_member("near", 2), dated_member("later", 12)]);
    engine.set_rate("near", RateState::Limited { not_before_ms: 500 });
    let mut metrics = ScenarioMetrics::default();
    for id in 1..=20 {
        metrics.complete(&mut engine, &request(id), 100 + id);
    }
    assert_eq!(metrics.served["later"], 20);
    assert!(
        engine
            .reserve(&request(21).with_owner("near"), 499)
            .is_err()
    );
    metrics.complete(&mut engine, &request(22).with_owner("near"), 500);
    metrics.complete(&mut engine, &request(23), 500);
    assert_eq!(metrics.served["near"], 2);
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn incomplete_latency_evidence_preserves_warm_sessions() {
    let mut engine = engine_with([candidate("steady", 0, 1), candidate("fast", 0, 1)]);
    record_ready_latency(&mut engine, "fast", 1, 100);
    for _ in 0..19 {
        engine.observe_first_output_latency("steady", "responses:gpt", Some(1_000), 100);
    }
    assert_eq!(
        engine
            .select(&preferred_request(1, "steady"), 100)
            .unwrap()
            .candidate_id,
        "steady",
    );
    engine.observe_first_output_latency("steady", "responses:gpt", Some(1_000), 100);
    assert_eq!(
        engine
            .select(&preferred_request(2, "steady"), 100)
            .unwrap()
            .candidate_id,
        "fast",
    );
    assert_eq!(
        engine
            .select(&request(3).with_owner("steady"), 100)
            .unwrap()
            .candidate_id,
        "steady",
    );
}

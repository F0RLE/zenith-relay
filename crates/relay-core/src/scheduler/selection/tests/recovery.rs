use super::*;
use crate::PoolRoutingMode;

fn recovering_pool() -> PoolScheduler {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("api-a"));
    scheduler.upsert(candidate("api-b"));
    let mut policy = scheduler.migrated_pool_routing();
    policy.mode = PoolRoutingMode::Smart;
    scheduler.set_pool_routing(policy).unwrap();
    scheduler.record_failure_at("api-a", 100);
    scheduler.record_failure_at("api-a", 100);
    scheduler
}

fn at(scheduler: &mut PoolScheduler, now_ms: u64) -> Option<Selection> {
    scheduler.select(SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &CandidateScope::default(),
        tried: &HashSet::new(),
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms,
    })
}

#[test]
fn smart_recovers_after_errors_without_a_refresh_or_restart() {
    for cooldown in [false, true] {
        let mut scheduler = recovering_pool();
        if cooldown {
            scheduler.set_cooldown("api-a", "*", 1_000);
        }
        assert_eq!(at(&mut scheduler, 1_001).unwrap().candidate_id, "api-b");
        let recovered = at(&mut scheduler, 60_100).unwrap();
        assert_eq!(recovered.candidate_id, "api-a");
        assert_eq!(recovered.half_open_probe, cooldown);
        let slot = scheduler
            .reserve_request("api-a", "gpt-5", 60_100, false)
            .unwrap();
        scheduler.record_success("api-a", "gpt-5", 60_100);
        assert!(scheduler.release_reservation(slot));
        assert_eq!(
            scheduler.candidate("api-a").unwrap().consecutive_failures,
            0
        );
    }
}

#[test]
fn decayed_failure_penalty_never_overrides_mandatory_eligibility() {
    let mut scheduler = recovering_pool();
    scheduler.set_cooldown("api-a", "*", 600_000);
    assert_eq!(at(&mut scheduler, 60_100).unwrap().candidate_id, "api-b");
    scheduler.clear_cooldown("api-a", "*");
    scheduler.candidates.get_mut("api-a").unwrap().quota = CandidateQuota::Exhausted;
    assert_eq!(at(&mut scheduler, 60_100).unwrap().candidate_id, "api-b");
    scheduler.candidates.get_mut("api-a").unwrap().quota = CandidateQuota::Unknown;
    scheduler.set_candidate_health("api-a", CandidateHealth::ReauthRequired);
    assert_eq!(at(&mut scheduler, 60_100).unwrap().candidate_id, "api-b");
}

#[test]
fn old_request_release_cannot_release_a_recovery_probe_for_the_same_model() {
    for scope in ["*", "gpt-5"] {
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("only"));
        let old = scheduler
            .reserve_request("only", "gpt-5", 1, false)
            .unwrap();
        scheduler.set_cooldown("only", scope, 99);
        let probe = scheduler
            .reserve_request("only", "gpt-5", 100, false)
            .unwrap();
        assert!(at(&mut scheduler, 100).is_none());
        assert!(scheduler.release_reservation(old));
        assert!(!scheduler.release_reservation(old));
        assert!(at(&mut scheduler, 100).is_none());
        assert_eq!(scheduler.active_request_count("only"), 1);
        assert!(scheduler.release_reservation(probe));
        assert!(at(&mut scheduler, 100).unwrap().half_open_probe);
    }
}

#[test]
fn old_probe_cannot_release_the_owner_of_a_new_cooldown() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    scheduler.set_cooldown("only", "*", 99);
    let old = scheduler
        .reserve_request("only", "gpt-5", 100, true)
        .unwrap();
    scheduler.set_cooldown("only", "*", 199);
    let current = scheduler
        .reserve_request("only", "gpt-5", 200, false)
        .unwrap();
    assert!(scheduler.release_reservation(old));
    assert!(at(&mut scheduler, 200).is_none());
    assert!(scheduler.release_reservation(current));
    assert!(at(&mut scheduler, 200).unwrap().half_open_probe);
}

#[test]
fn repeating_a_cooldown_does_not_cancel_its_running_probe() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    scheduler.set_cooldown("only", "gpt-5", 99);
    let probe = scheduler
        .reserve_request("only", "gpt-5", 100, false)
        .unwrap();
    for retry_at in [99, 98] {
        scheduler.set_cooldown("only", "gpt-5", retry_at);
        assert!(at(&mut scheduler, 100).is_none());
        assert!(scheduler
            .reserve_request("only", "gpt-5", 100, false)
            .is_none());
    }
    scheduler.release_reservation(probe);
    assert!(at(&mut scheduler, 100).unwrap().half_open_probe);
}

#[test]
fn success_from_an_older_request_does_not_release_a_running_probe() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    let old = scheduler
        .reserve_request("only", "gpt-5", 1, false)
        .unwrap();
    scheduler.set_cooldown("only", "*", 99);
    let probe = scheduler
        .reserve_request("only", "gpt-5", 100, false)
        .unwrap();
    scheduler.record_success("only", "gpt-5", 101);
    scheduler.release_reservation(old);
    assert!(at(&mut scheduler, 101).is_none());
    scheduler.release_reservation(probe);
    assert!(!at(&mut scheduler, 101).unwrap().half_open_probe);
}

#[test]
fn recovery_probe_is_a_capacity_wait_but_auth_and_quota_blocks_are_not() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    scheduler.set_cooldown("only", "*", 99);
    let probe = scheduler
        .reserve_request("only", "gpt-5", 100, false)
        .unwrap();
    let busy = |scheduler: &mut PoolScheduler, tried: &HashSet<String>| {
        scheduler.capacity_blocked(
            SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried,
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 100,
            },
            false,
        )
    };
    assert!(busy(&mut scheduler, &HashSet::new()));
    assert!(!busy(&mut scheduler, &HashSet::from(["only".into()])));
    scheduler.candidates.get_mut("only").unwrap().quota = CandidateQuota::Exhausted;
    assert!(!busy(&mut scheduler, &HashSet::new()));
    scheduler.candidates.get_mut("only").unwrap().quota = CandidateQuota::Unknown;
    scheduler.set_candidate_health("only", CandidateHealth::ReauthRequired);
    assert!(!busy(&mut scheduler, &HashSet::new()));
    scheduler.release_reservation(probe);
}

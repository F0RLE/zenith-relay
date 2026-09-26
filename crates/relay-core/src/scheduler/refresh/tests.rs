use super::*;
use crate::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind};

fn coordinator() -> RefreshCoordinator {
    RefreshCoordinator::new(RefreshLimits {
        start_spacing_ms: 0,
        origin_spacing_ms: 0,
        minimum_interval_ms: 0,
        ..RefreshLimits::default()
    })
    .unwrap()
}

fn identity(revision: u64) -> RefreshIdentity {
    RefreshIdentity::new("member-a", revision, 1)
}

#[test]
fn quota_reset_delay_uses_the_earliest_future_window_and_stable_jitter() {
    let window = |kind, reset_at_ms| QuotaWindow {
        kind,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(0),
        explicitly_full: Some(false),
        reset_at_ms: Some(reset_at_ms),
        window_minutes: None,
        observed_at_ms: 90_000,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
    };
    let mut quota = QuotaSnapshot {
        primary: Some(window(QuotaWindowKind::Primary, 100_000)),
        secondary: Some(window(QuotaWindowKind::Secondary, 120_000)),
        ..QuotaSnapshot::default()
    };
    let delay = quota_reset_delay("synthetic", &quota, 90_000).unwrap();
    assert!((15_000..25_000).contains(&delay));
    assert_eq!(quota_reset_delay("synthetic", &quota, 90_000), Some(delay));
    assert!(quota_reset_delay("synthetic", &quota, 120_000).is_none());
    quota.primary = None;
    assert!((35_000..45_000).contains(&quota_reset_delay("synthetic", &quota, 90_000).unwrap()));
}

#[test]
fn passive_quota_requires_a_recent_window_not_only_an_updated_timestamp() {
    let mut quota = QuotaSnapshot {
        updated_at_ms: Some(99_000),
        primary: Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(2_000),
            explicitly_full: None,
            reset_at_ms: None,
            window_minutes: None,
            observed_at_ms: 99_000,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }),
        ..QuotaSnapshot::default()
    };
    assert_eq!(passive_quota_age_ms(&quota, 100_000), Some(1_000));
    quota.secondary = Some(QuotaWindow {
        kind: QuotaWindowKind::Secondary,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(3_000),
        explicitly_full: None,
        reset_at_ms: None,
        window_minutes: None,
        observed_at_ms: 1,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
    });
    assert_eq!(passive_quota_age_ms(&quota, 100_000), Some(99_999));
    quota.secondary = None;
    quota.primary = None;
    assert_eq!(passive_quota_age_ms(&quota, 100_000), None);
    quota.primary = Some(QuotaWindow {
        kind: QuotaWindowKind::Primary,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(2_000),
        explicitly_full: None,
        reset_at_ms: None,
        window_minutes: None,
        observed_at_ms: 99_000,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
    });
    quota.error = Some(crate::quota::QuotaErrorState::new(
        "synthetic_failure",
        100_000,
    ));
    assert_eq!(passive_quota_age_ms(&quota, 100_000), None);
}

#[test]
fn passive_quota_defers_only_automatic_poll_and_preserves_reset_and_manual_work() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 1_000, true, true);
    coordinator.register(member.clone(), RefreshKind::Models, 1_000, true, true);
    assert!(coordinator.observe_passive_quota(&member, 1_000, 0, 1_000));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(1_000 + 5 * 60_000)
    );
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Models),
        Some(1_000)
    );
    assert!(!coordinator.observe_passive_quota(&member, 1_000, 0, 1_000));
    assert!(!coordinator.observe_passive_quota(&member, 1_000 + 5 * 60_000, 5 * 60_000, 1_000));

    assert!(coordinator.schedule_event(&member, RefreshKind::Quota, 10_000));
    assert!(coordinator.observe_passive_quota(&member, 2_000, 0, 2_000));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(10_000)
    );
    assert!(coordinator.request_now(&member, RefreshKind::Quota, 3_000));
    assert!(coordinator.observe_passive_quota(&member, 4_000, 0, 4_000));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(3_000)
    );
    assert!(coordinator.claim_due(4_000).is_some());
}

#[test]
fn passive_quota_during_a_failed_read_defers_follow_up_but_not_provider_floor() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 1_000, true, true);
    let job = coordinator.claim_due(1_000).unwrap();
    assert!(coordinator.observe_passive_quota(&member, 1_100, 0, 1_100));
    coordinator.defer_until(&member, RefreshKind::Quota, 400_000);
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::FailedRetryAt(120_000), 1_200),
        RefreshCompletion::Applied {
            next_due_ms: Some(400_000)
        }
    );
    assert_eq!(
        coordinator.freshness(&member, RefreshKind::Quota, 1_200),
        RefreshFreshness::Fresh { as_of_ms: 1_100 }
    );
    assert_eq!(coordinator.claim_due(399_999), None);
    assert!(coordinator.claim_due(400_000).is_some());
}

#[test]
fn a_newer_passive_quota_during_successful_read_is_not_shifted_to_job_end() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 1_000, true, true);
    let job = coordinator.claim_due(1_000).unwrap();
    assert!(coordinator.observe_passive_quota(&member, 1_100, 0, 1_700_000_000_000));
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 120_000),
        RefreshCompletion::Applied {
            next_due_ms: Some(301_100)
        }
    );
    assert_eq!(
        coordinator.freshness(&member, RefreshKind::Quota, 301_099),
        RefreshFreshness::Fresh { as_of_ms: 1_100 }
    );
    assert_eq!(
        coordinator.freshness(&member, RefreshKind::Quota, 301_100),
        RefreshFreshness::Stale { as_of_ms: 1_100 }
    );
}

#[test]
fn passive_quota_before_the_service_started_uses_only_remaining_freshness() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 500, true, true);
    assert!(coordinator.observe_passive_quota(&member, 500, 120_000, 1_700_000_000_000));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(180_500)
    );
    assert_eq!(
        coordinator.freshness(&member, RefreshKind::Quota, 180_499),
        RefreshFreshness::Fresh { as_of_ms: 0 }
    );
    assert_eq!(
        coordinator.freshness(&member, RefreshKind::Quota, 180_500),
        RefreshFreshness::Stale { as_of_ms: 0 }
    );
    assert!(coordinator.observe_passive_quota(&member, 501, 0, 1_700_000_001_000));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(300_501)
    );
}

#[test]
fn activity_changes_recalculate_a_passive_quota_cadence_from_actual_age() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 500, false, true);
    assert!(coordinator.observe_passive_quota(&member, 500, 120_000, 1_700_000_000_000));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(780_500)
    );
    assert!(coordinator.set_active(&member, true, 1_500));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(180_500)
    );
    assert!(coordinator.set_active(&member, false, 2_500));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Quota),
        Some(780_500)
    );
}

#[test]
fn a_future_reset_event_survives_an_earlier_automatic_poll() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 0, true, true);
    assert!(coordinator.schedule_event(&member, RefreshKind::Quota, 100_000));
    let job = coordinator.claim_due(0).unwrap();
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 10),
        RefreshCompletion::Applied {
            next_due_ms: Some(100_000)
        }
    );
    assert!(coordinator.claim_due(99_999).is_none());
    assert_eq!(
        coordinator.claim_due(100_000).unwrap().kind,
        RefreshKind::Quota
    );
}

#[test]
fn active_and_idle_cadences_match_the_single_test_profile() {
    assert_eq!(RefreshKind::Auth.interval_ms(true), None);
    assert_eq!(RefreshKind::Quota.interval_ms(true), Some(5 * 60 * 1_000));
    assert_eq!(RefreshKind::Quota.interval_ms(false), Some(15 * 60 * 1_000));
    assert_eq!(
        RefreshKind::Models.interval_ms(true),
        Some(8 * 60 * 60 * 1_000)
    );
    assert_eq!(
        RefreshKind::Balance.interval_ms(false),
        Some(30 * 60 * 1_000)
    );
    assert_eq!(
        RefreshKind::Prices.interval_ms(true),
        Some(24 * 60 * 60 * 1_000)
    );
}

#[test]
fn auth_is_expiry_driven_and_does_not_start_a_polling_loop_after_success() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.schedule_at(member.clone(), RefreshKind::Auth, 40_000, false);
    assert_eq!(coordinator.claim_due(39_999), None);
    let job = coordinator.claim_due(40_000).unwrap();
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 40_100),
        RefreshCompletion::Applied { next_due_ms: None }
    );
    assert_eq!(coordinator.claim_due(40_100 + 24 * 60 * 60 * 1_000), None);
}

#[test]
fn repeated_dirty_events_join_one_in_flight_job_and_requeue_after_completion() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 100, true, true);
    let job = coordinator.claim_due(100).unwrap();
    assert_eq!(coordinator.pending_jobs(), 1);
    assert!(coordinator.mark_dirty(&member, RefreshKind::Quota, 101));
    assert!(coordinator.mark_dirty(&member, RefreshKind::Quota, 102));
    assert_eq!(coordinator.claim_due(102), None);
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 200),
        RefreshCompletion::Applied {
            next_due_ms: Some(200),
        }
    );
    assert_eq!(coordinator.claim_due(200).unwrap().id, RefreshJobId(2));
}

#[test]
fn a_dirty_event_consumed_by_the_claim_does_not_poll_twice() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Quota, 0, true, false);
    assert!(coordinator.mark_dirty(&member, RefreshKind::Quota, 100));
    let job = coordinator.claim_due(100).unwrap();
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 200),
        RefreshCompletion::Applied {
            next_due_ms: Some(200 + 5 * 60 * 1_000),
        }
    );
    assert_eq!(coordinator.claim_due(200), None);
}

#[test]
fn auth_expiry_scheduled_during_refresh_survives_success() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.schedule_at(member.clone(), RefreshKind::Auth, 100, true);
    let job = coordinator.claim_due(100).unwrap();
    coordinator.schedule_at(member.clone(), RefreshKind::Auth, 10_000, true);
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 200),
        RefreshCompletion::Applied {
            next_due_ms: Some(10_000),
        }
    );
    assert_eq!(coordinator.claim_due(9_999), None);
    assert_eq!(coordinator.claim_due(10_000).unwrap().identity, member);
}

#[test]
fn provider_retry_after_wins_over_a_dirty_event() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Balance, 100, true, true);
    let job = coordinator.claim_due(100).unwrap();
    coordinator.mark_dirty(&member, RefreshKind::Balance, 101);
    coordinator.register(member.clone(), RefreshKind::Balance, 102, true, true);
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::FailedRetryAt(10_000), 200),
        RefreshCompletion::Applied {
            next_due_ms: Some(10_000),
        }
    );
    assert_eq!(coordinator.claim_due(9_999), None);
}

#[test]
fn events_after_failed_completion_cannot_advance_provider_retry_after() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Balance, 100, false, true);
    let job = coordinator.claim_due(100).unwrap();
    coordinator.complete(&job, RefreshOutcome::FailedRetryAt(1_000_000), 200);
    for now in 201..210 {
        coordinator.mark_dirty(&member, RefreshKind::Balance, now);
        coordinator.schedule_at(member.clone(), RefreshKind::Balance, now, true);
        coordinator.register(member.clone(), RefreshKind::Balance, now, true, true);
        coordinator.set_active(&member, true, now);
        assert_eq!(
            coordinator.next_due(&member, RefreshKind::Balance),
            Some(1_000_000)
        );
        assert_eq!(coordinator.claim_due(now), None);
    }
    assert_eq!(coordinator.claim_due(999_999), None);
    let retry = coordinator.claim_due(1_000_000).unwrap();
    assert_eq!(retry.due_at_ms, 1_000_000);
    coordinator.complete(&retry, RefreshOutcome::Success, 1_000_100);
    coordinator.mark_dirty(&member, RefreshKind::Balance, 1_000_101);
    assert!(coordinator.claim_due(1_000_101).is_some());
}

#[test]
fn old_revision_completion_cannot_publish_into_a_new_identity() {
    let mut coordinator = coordinator();
    let old = identity(1);
    let new = identity(2);
    coordinator.register(old.clone(), RefreshKind::Models, 0, true, true);
    let job = coordinator.claim_due(0).unwrap();
    assert_eq!(coordinator.invalidate(&old), 1);
    coordinator.register(new.clone(), RefreshKind::Models, 1, true, true);
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 2),
        RefreshCompletion::Stale
    );
    assert_eq!(coordinator.next_due(&new, RefreshKind::Models), Some(1));
    assert_eq!(coordinator.claim_due(1).unwrap().identity, new);
}

#[test]
fn active_transition_accelerates_idle_work_without_creating_a_second_job() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Balance, 0, false, false);
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Balance),
        Some(30 * 60 * 1_000)
    );
    assert!(coordinator.set_active(&member, true, 1_000));
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Balance),
        Some(5 * 60 * 1_000 + 1_000)
    );
}

#[test]
fn failed_refresh_uses_provider_retry_at_but_does_not_apply_an_older_result() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Auth, 0, true, true);
    let job = coordinator.claim_due(0).unwrap();
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::FailedRetryAt(10_000), 500),
        RefreshCompletion::Applied {
            next_due_ms: Some(10_000)
        }
    );
    assert_eq!(coordinator.claim_due(9_999), None);
    let retry = coordinator.claim_due(10_000).unwrap();
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 11_000),
        RefreshCompletion::UnknownJob
    );
    assert_eq!(
        coordinator.complete(&retry, RefreshOutcome::Success, 11_000),
        RefreshCompletion::Applied { next_due_ms: None }
    );
}

#[test]
fn ordinary_work_cannot_consume_auth_reserve_at_runtime_or_origin() {
    let mut coordinator = coordinator();
    for n in 0..20 {
        assert!(coordinator.register_origin(
            RefreshIdentity::new(format!("q{n}"), 1, 1),
            RefreshKind::Quota,
            format!("origin-{}", n / 4),
            0,
            false,
            true
        ));
    }
    let mut running = Vec::new();
    while let Some(job) = coordinator.claim_due(0) {
        running.push(job);
    }
    assert_eq!(running.len(), 7);
    let mut by_origin = BTreeMap::new();
    for job in coordinator.jobs.values() {
        *by_origin.entry(&job.origin).or_insert(0) += 1;
    }
    assert!(by_origin.values().all(|count| *count <= 2));
    coordinator.register_origin(
        RefreshIdentity::new("auth", 1, 1),
        RefreshKind::Auth,
        "origin-0".into(),
        0,
        true,
        true,
    );
    assert_eq!(coordinator.claim_due(0).unwrap().kind, RefreshKind::Auth);
    assert!(coordinator.claim_due(0).is_none());
}

#[test]
fn origin_and_global_start_spacing_limit_resume_bursts() {
    let mut coordinator = RefreshCoordinator::default();
    for n in 0..12 {
        coordinator.register_origin(
            RefreshIdentity::new(format!("m{n:02}"), 1, 1),
            RefreshKind::Models,
            if n < 6 { "a" } else { "b" }.into(),
            0,
            true,
            true,
        );
    }
    let first = coordinator.claim_due(1_000_000).unwrap();
    assert!(coordinator.claim_due(1_000_000).is_none());
    assert_eq!(coordinator.next_wake(), Some(1_000_050));
    let second = coordinator.claim_due(1_000_050).unwrap();
    assert_ne!(
        coordinator.jobs[&first.id].origin,
        coordinator.jobs[&second.id].origin
    );
    assert!(coordinator.claim_due(1_000_100).is_none());
    assert_eq!(coordinator.next_wake(), Some(1_000_250));
}

#[test]
fn continuous_high_classes_do_not_starve_lower_kinds() {
    let mut coordinator = coordinator();
    let member = identity(1);
    let kinds = [
        RefreshKind::Auth,
        RefreshKind::Quota,
        RefreshKind::Models,
        RefreshKind::Balance,
        RefreshKind::Metadata,
        RefreshKind::Prices,
    ];
    for kind in kinds {
        coordinator.register(member.clone(), kind, 0, true, true);
    }
    let mut seen = BTreeMap::new();
    for _ in 0..80 {
        let job = coordinator.claim_due(0).unwrap();
        *seen.entry(job.kind).or_insert(0) += 1;
        coordinator.mark_dirty(&member, job.kind, 0);
        coordinator.complete(&job, RefreshOutcome::Success, 0);
    }
    for kind in kinds {
        assert!(seen[&kind] >= 10);
    }
    assert_eq!(seen[&RefreshKind::Auth], 20);
}

#[test]
fn unsupported_stops_periodic_and_dirty_but_manual_can_recheck() {
    let mut coordinator = coordinator();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Balance, 0, true, true);
    let job = coordinator.claim_due(0).unwrap();
    coordinator.complete(&job, RefreshOutcome::Unsupported, 1);
    assert_eq!(
        coordinator.freshness(&member, RefreshKind::Balance, 2),
        RefreshFreshness::Unsupported
    );
    assert!(!coordinator.mark_dirty(&member, RefreshKind::Balance, 2));
    coordinator.register(member.clone(), RefreshKind::Balance, 3, true, true);
    assert_eq!(coordinator.next_wake(), None);
    coordinator.request_now(&member, RefreshKind::Balance, 4);
    let retry = coordinator.claim_due(4).unwrap();
    coordinator.complete(&retry, RefreshOutcome::Success, 5);
    assert_eq!(
        coordinator.freshness(&member, RefreshKind::Balance, 5),
        RefreshFreshness::Fresh { as_of_ms: 5 }
    );
}

#[test]
fn no_progress_and_minimum_intervals_cannot_be_bypassed_by_dirty_or_manual() {
    let mut coordinator = RefreshCoordinator::default();
    let member = identity(1);
    coordinator.register(member.clone(), RefreshKind::Auth, 0, true, true);
    let job = coordinator.claim_due(0).unwrap();
    coordinator.mark_dirty(&member, RefreshKind::Auth, 1);
    coordinator.complete(&job, RefreshOutcome::NoProgress, 2);
    coordinator.request_now(&member, RefreshKind::Auth, 3);
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Auth),
        Some(5_002)
    );
    let second = coordinator.claim_due(5_002).unwrap();
    coordinator.complete(&second, RefreshOutcome::NoProgress, 5_003);
    assert_eq!(
        coordinator.next_due(&member, RefreshKind::Auth),
        Some(15_003)
    );
    let third = coordinator.claim_due(15_003).unwrap();
    coordinator.complete(&third, RefreshOutcome::Success, 15_004);
    coordinator.request_now(&member, RefreshKind::Auth, 15_005);
    assert!(coordinator.claim_due(16_003).is_none());
    assert!(coordinator.claim_due(16_004).is_some());
}

#[test]
fn bounded_registration_and_invalid_completion_do_not_lose_live_capacity() {
    let mut coordinator = RefreshCoordinator::new(RefreshLimits {
        max_entries: 1,
        ..RefreshLimits::default()
    })
    .unwrap();
    assert!(coordinator.register(identity(1), RefreshKind::Quota, 0, true, true));
    assert!(!coordinator.register(identity(2), RefreshKind::Quota, 0, true, true));
    let job = coordinator.claim_due(0).unwrap();
    let mut forged = job.clone();
    forged.kind = RefreshKind::Models;
    assert_eq!(
        coordinator.complete(&forged, RefreshOutcome::Success, 1),
        RefreshCompletion::Stale
    );
    coordinator.invalidate(&identity(1));
    assert_eq!(coordinator.pending_jobs(), 1);
    assert_eq!(
        coordinator.complete(&job, RefreshOutcome::Success, 2),
        RefreshCompletion::Stale
    );
    assert_eq!(coordinator.pending_jobs(), 0);
}

#[test]
fn received_hint_survives_host_persistence_failure_or_a_newer_passive_value() {
    for outcome in [
        RefreshOutcome::Success,
        RefreshOutcome::FailedRetryAt(60_000),
    ] {
        let mut coordinator = coordinator();
        let member = identity(1);
        coordinator.register(member.clone(), RefreshKind::Quota, 0, true, true);
        let job = coordinator.claim_due(0).unwrap();
        coordinator.defer_until(&member, RefreshKind::Quota, 7_200_000);
        coordinator.complete(&job, outcome, 2);
        coordinator.request_now(&member, RefreshKind::Quota, 3);
        assert_eq!(
            coordinator.next_due(&member, RefreshKind::Quota),
            Some(7_200_000)
        );
        assert!(coordinator.claim_due(7_199_999).is_none());
    }
}

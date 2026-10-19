//! Synthetic lifecycle regressions and opt-in local scheduling measurements.

use super::*;
use std::time::Instant;

fn member(id: &str) -> RotationCandidate {
    let mut member = RotationCandidate::new(id, "text", "text-model").with_capacity(id, 4);
    member.add_route("image", "image-model", RotationOperation::Image);
    member.add_route("compact", "text-model", RotationOperation::Compaction);
    member
}

#[test]
fn mixed_operations_and_lifecycle_churn_release_all_state() {
    let mut engine = RotationEngine::default();
    let ids = ["a", "b", "c", "d"];
    for id in ids {
        engine.upsert(member(id)).unwrap();
    }
    let mut pending = Vec::<(u64, RotationLease, RequestBudget)>::new();
    let mut served = BTreeMap::<String, usize>::new();
    for step in 0..4_000u64 {
        let now = 1_000 + step * 10;
        let mut index = 0;
        while index < pending.len() {
            if pending[index].0 > now {
                index += 1;
                continue;
            }
            let (_, lease, budget) = pending.swap_remove(index);
            let unknown = lease.request_id.0.is_multiple_of(13);
            let settlement = engine
                .settle(
                    lease.lease_id,
                    AttemptObservation {
                        execution: if unknown {
                            ExecutionObservation::unknown()
                        } else {
                            ExecutionObservation::committed()
                        },
                        health: if unknown {
                            HealthObservation::Unknown
                        } else {
                            HealthObservation::Success
                        },
                    },
                    &budget,
                    now,
                )
                .unwrap();
            assert!(matches!(settlement.retry, RetryDecision::Stop(_)));
        }
        if step.is_multiple_of(97) {
            engine.set_enabled("b", step.is_multiple_of(194));
        }
        if step.is_multiple_of(211) {
            let draining = !step.is_multiple_of(422);
            engine.sync_candidate_state(
                "c",
                true,
                draining,
                AuthState::Ready,
                QuotaState::Unknown,
                RateState::Ready,
            );
        }
        if step.is_multiple_of(257) {
            engine.set_quota(
                "d",
                QuotaState::Exhausted {
                    reset_at_ms: Some(now + 50),
                },
            );
        } else if step % 257 == 8 {
            engine.set_quota("d", QuotaState::Available);
            engine.set_quota_remaining("d", Some(8_000), Some(now));
        }
        if step.is_multiple_of(401) {
            engine.remove("b").unwrap();
            engine.upsert(member("b")).unwrap();
        }
        if step.is_multiple_of(503) {
            engine.set_mode(if step.is_multiple_of(1_006) {
                RotationMode::Automatic
            } else {
                RotationMode::RoundRobin
            });
        }
        let (route, model, operation) = match step % 3 {
            0 => ("text", "text-model", RotationOperation::Text),
            1 => ("image", "image-model", RotationOperation::Image),
            _ => ("compact", "text-model", RotationOperation::Compaction),
        };
        let request =
            RotationRequest::new(RequestId(step + 1), route, model).with_operation(operation);
        if let Ok(lease) = engine.reserve(&request, now) {
            *served.entry(lease.candidate_id.clone()).or_default() += 1;
            let mut budget = RequestBudget::default_for(request.request_id);
            engine.begin_dispatch(lease.lease_id, &mut budget).unwrap();
            if step.is_multiple_of(11) {
                engine.cancel(lease.lease_id, &budget, now).unwrap();
            } else {
                pending.push((now + 10 + (step % 9) * 20, lease, budget));
            }
        }
        assert_eq!(engine.active_leases(), pending.len());
        for id in ids {
            assert!(engine.capacity_in_flight(id) <= 4);
        }
        assert!(engine.circuits.len() <= 12);
        assert!(engine.health.len() <= 12);
        assert!(engine.rotation_credit.len() <= 12);
        assert!(engine.round_robin_cursor.len() <= 3);
    }
    for (_, lease, budget) in pending {
        engine.cancel(lease.lease_id, &budget, 50_000).unwrap();
    }
    assert_eq!(engine.active_leases(), 0);
    assert!(engine.capacity_in_flight.is_empty());
    for id in ids {
        assert!(
            served[id] > 50,
            "member {id} did not receive useful traffic"
        );
        assert_eq!(engine.in_flight(id), 0);
        engine.remove(id).unwrap();
    }
    assert!(engine.circuits.is_empty());
    assert!(engine.health.is_empty());
    assert!(engine.rotation_credit.is_empty());
    assert!(engine.round_robin_cursor.is_empty());
}

#[test]
fn round_robin_discards_cursors_only_after_the_last_route_is_removed() {
    let mut engine = RotationEngine::default();
    engine.set_mode(RotationMode::RoundRobin);
    for revision in 0..100 {
        let route = format!("route-{revision}");
        for id in ["a", "b"] {
            engine
                .upsert(RotationCandidate::new(id, &route, "model"))
                .unwrap();
        }
        let request = RotationRequest::new(RequestId(revision), &route, "model");
        let lease = engine.reserve(&request, 0).unwrap();
        assert!(engine.release_unstarted(lease.lease_id));
        assert_eq!(engine.round_robin_cursor.len(), 1);
    }
    engine.remove("a").unwrap();
    assert_eq!(engine.round_robin_cursor.len(), 1);
    engine.remove("b").unwrap();
    assert!(engine.round_robin_cursor.is_empty());
}

#[test]
#[ignore = "local timing probe; no provider I/O or machine-dependent timing assertions"]
fn scheduling_latency_under_contention() {
    for members in [2, 16, 64] {
        let mut engine = RotationEngine::default();
        for id in 0..members {
            engine.upsert(member(&format!("member-{id}"))).unwrap();
        }
        let engine = Mutex::new(engine);
        let barrier = std::sync::Barrier::new(4);
        let samples = std::thread::scope(|scope| {
            let workers = (0..4)
                .map(|worker| {
                    let engine = &engine;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let mut samples = Vec::new();
                        barrier.wait();
                        for sequence in 0..1_000 {
                            let request = RotationRequest::new(
                                RequestId(worker * 1_000 + sequence),
                                "text",
                                "text-model",
                            );
                            let waiting = Instant::now();
                            let mut engine = engine.lock().unwrap();
                            let lock_wait = waiting.elapsed();
                            let selecting = Instant::now();
                            let lease = engine.reserve(&request, 0).unwrap();
                            let selection = selecting.elapsed();
                            assert!(engine.release_unstarted(lease.lease_id));
                            drop(engine);
                            samples.push((lock_wait.as_nanos(), selection.as_nanos()));
                        }
                        samples
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        for (name, mut times) in [
            (
                "lock_wait",
                samples.iter().map(|sample| sample.0).collect::<Vec<_>>(),
            ),
            (
                "reserve",
                samples.iter().map(|sample| sample.1).collect::<Vec<_>>(),
            ),
        ] {
            times.sort_unstable();
            eprintln!(
                "members={members} {name}_ns p50={} p95={} p99={}",
                times[times.len() / 2],
                times[times.len() * 95 / 100],
                times[times.len() * 99 / 100],
            );
        }
        assert_eq!(engine.lock().unwrap().active_leases(), 0);
    }
}

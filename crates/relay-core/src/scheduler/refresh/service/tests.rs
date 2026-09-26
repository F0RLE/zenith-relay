use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{mpsc, Notify};

fn registration(revision: u64, automatic: bool) -> RefreshRegistration {
    RefreshRegistration {
        identity: RefreshIdentity::new("account:test", revision, 1),
        kind: RefreshKind::Quota,
        origin: "https://provider.example.test".into(),
        active: false,
        automatic,
        due_now: false,
    }
}

#[tokio::test(start_paused = true)]
async fn reset_event_accelerates_quota_without_changing_activity_or_models() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let identity = registration(1, true).identity;
    for kind in [RefreshKind::Quota, RefreshKind::Models] {
        let mut entry = registration(1, true);
        entry.kind = kind;
        service
            .register(entry, |_| {
                Box::pin(async {
                    RefreshResult {
                        value: (),
                        outcome: RefreshOutcome::Success,
                    }
                })
            })
            .unwrap();
        service.request(&identity, kind).await.unwrap();
    }
    let due_before = service
        .state
        .lock()
        .unwrap()
        .coordinator
        .next_due(&identity, RefreshKind::Models);
    assert!(service.schedule_after(&identity, RefreshKind::Quota, 5_000));
    {
        let state = service.state.lock().unwrap();
        assert_eq!(
            state.coordinator.next_due(&identity, RefreshKind::Quota),
            Some(service.now_ms() + 5_000)
        );
        assert_eq!(
            state.coordinator.next_due(&identity, RefreshKind::Models),
            due_before
        );
        assert!(state
            .coordinator
            .entries
            .values()
            .all(|entry| !entry.active));
    }
    service.respect_retry_after(&identity, RefreshKind::Quota, 60_000);
    assert!(service.schedule_after(&identity, RefreshKind::Quota, 0));
    assert_eq!(
        service
            .state
            .lock()
            .unwrap()
            .coordinator
            .next_due(&identity, RefreshKind::Quota),
        Some(service.now_ms() + 60_000)
    );
    service.shutdown().await;
}

#[tokio::test]
async fn repeated_activity_and_unregistered_members_do_not_wake_inventory_scans() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    service
        .register(registration(1, false), |_| {
            Box::pin(async {
                RefreshResult {
                    value: (),
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    let mut changed = service.changed.subscribe();
    service.set_member_active("source:unregistered");
    assert!(!changed.has_changed().unwrap());
    service.set_member_active("account:test");
    assert!(changed.has_changed().unwrap());
    changed.borrow_and_update();
    service.set_member_active("account:test");
    assert!(!changed.has_changed().unwrap());
    service.set_active(&registration(1, false).identity, false);
    assert!(changed.has_changed().unwrap());
    service.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn becoming_eligible_schedules_once_without_resetting_every_reconciliation() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let work = |_| {
        Box::pin(async {
            RefreshResult {
                value: (),
                outcome: RefreshOutcome::Success,
            }
        }) as BoxFuture<'static, RefreshResult<()>>
    };
    service.register(registration(1, false), work).unwrap();
    service
        .request(&registration(1, false).identity, RefreshKind::Quota)
        .await
        .unwrap();
    let mut activated = registration(1, true);
    activated.due_now = true;
    service.register(activated, work).unwrap();
    let identity = registration(1, true).identity;
    assert!(service
        .state
        .lock()
        .unwrap()
        .coordinator
        .next_due(&identity, RefreshKind::Quota)
        .is_some());
    service
        .request(&identity, RefreshKind::Quota)
        .await
        .unwrap();
    let scheduled = service
        .state
        .lock()
        .unwrap()
        .coordinator
        .next_due(&identity, RefreshKind::Quota);
    let mut unchanged = registration(1, true);
    unchanged.due_now = true;
    service.register(unchanged, work).unwrap();
    assert_eq!(
        service
            .state
            .lock()
            .unwrap()
            .coordinator
            .next_due(&identity, RefreshKind::Quota),
        scheduled
    );
    service.shutdown().await;
}

#[tokio::test]
async fn retiring_member_unblocks_waiters_but_retains_running_traffic_capacity() {
    let service = RefreshService::new(RefreshLimits {
        concurrent: 1,
        per_origin: 1,
        reserved_auth: 0,
        start_spacing_ms: 0,
        origin_spacing_ms: 0,
        ..RefreshLimits::default()
    })
    .unwrap();
    let release = Arc::new(Notify::new());
    let (started, mut starts) = mpsc::unbounded_channel();
    let unblock = release.clone();
    service
        .register(registration(1, false), move |_| {
            let (release, started) = (unblock.clone(), started.clone());
            Box::pin(async move {
                started.send(()).unwrap();
                release.notified().await;
                RefreshResult {
                    value: 1,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    let caller_service = service.clone();
    let caller = tokio::spawn(async move {
        caller_service
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await
    });
    starts.recv().await.unwrap();
    assert!(service.remove_member("account:test"));
    assert_eq!(caller.await.unwrap(), Err(RefreshWaitError::Stale));
    assert_eq!(service.state.lock().unwrap().coordinator.pending_jobs(), 1);
    service
        .register(registration(2, true), |_| {
            Box::pin(async {
                RefreshResult {
                    value: 2,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    assert!(service.schedule_after(&registration(2, true).identity, RefreshKind::Quota, 0));
    assert_eq!(service.state.lock().unwrap().coordinator.next_wake(), None);
    release.notify_one();
    assert_eq!(
        *service
            .request(&registration(2, true).identity, RefreshKind::Quota)
            .await
            .unwrap(),
        2
    );
    service.shutdown().await;
}

#[tokio::test]
async fn completion_notification_observes_released_single_flight() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let identity = registration(1, false).identity;
    let mut progress = service.progress();
    let release = Arc::new(Notify::new());
    let unblock = release.clone();
    service
        .register(registration(1, false), move |_| {
            let release = unblock.clone();
            Box::pin(async move {
                release.notified().await;
                RefreshResult {
                    value: (),
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    let caller_service = service.clone();
    let caller = tokio::spawn(async move {
        caller_service
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await
    });
    progress.changed().await.unwrap();
    assert!(service.in_flight(&identity, RefreshKind::Quota));
    release.notify_one();
    caller.await.unwrap().unwrap();
    progress.changed().await.unwrap();
    assert!(!service.in_flight(&identity, RefreshKind::Quota));
    service.shutdown().await;
}

#[tokio::test]
async fn a_hundred_manual_callers_join_and_canceling_the_first_keeps_shared_work() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let (started, mut requests) = mpsc::unbounded_channel();
    let (observed, unblock) = (calls.clone(), release.clone());
    service
        .register(registration(1, false), move |_| {
            let (calls, release, started) = (observed.clone(), unblock.clone(), started.clone());
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                started.send(()).unwrap();
                release.notified().await;
                RefreshResult {
                    value: 42,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    let mut tasks = Vec::new();
    for _ in 0..100 {
        let service = service.clone();
        tasks.push(tokio::spawn(async move {
            service
                .request(&registration(1, false).identity, RefreshKind::Quota)
                .await
        }));
    }
    requests.recv().await.unwrap();
    // Wait for subscriptions, not a guessed number of executor yields.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let joined = service
                .state
                .lock()
                .unwrap()
                .entries
                .values()
                .filter_map(|entry| entry.result.as_ref())
                .map(watch::Sender::receiver_count)
                .sum::<usize>();
            if joined == 100 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tasks.remove(0).abort();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.notify_one();
    for task in tasks {
        assert_eq!(*task.await.unwrap().unwrap(), 42);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    service.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn retry_hint_survives_manual_requests_and_wall_clock_is_not_used() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let (started, mut requests) = mpsc::unbounded_channel();
    let clock = Arc::downgrade(&service);
    service
        .register(registration(1, false), move |_| {
            let (started, clock) = (started.clone(), clock.clone());
            Box::pin(async move {
                let now = clock.upgrade().unwrap().now_ms();
                started.send(now).unwrap();
                RefreshResult {
                    value: 1,
                    outcome: RefreshOutcome::FailedRetryAt(now + 60_000),
                }
            })
        })
        .unwrap();
    service
        .request(&registration(1, false).identity, RefreshKind::Quota)
        .await
        .unwrap();
    assert_eq!(requests.recv().await, Some(0));
    let request_service = service.clone();
    let request = tokio::spawn(async move {
        request_service
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(59_999)).await;
    tokio::task::yield_now().await;
    assert!(requests.try_recv().is_err());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(requests.recv().await, Some(60_000));
    request.await.unwrap().unwrap();
    service.shutdown().await;
}

#[tokio::test]
async fn replacement_releases_old_waiters_without_publishing_old_results() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let (started, mut requests) = mpsc::unbounded_channel();
    service
        .register(registration(1, false), move |_| {
            let started = started.clone();
            Box::pin(async move {
                started.send(()).unwrap();
                std::future::pending::<RefreshResult<usize>>().await
            })
        })
        .unwrap();
    let old_service = service.clone();
    let old = tokio::spawn(async move {
        old_service
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await
    });
    requests.recv().await.unwrap();
    service
        .register(registration(2, false), |_| {
            Box::pin(async {
                RefreshResult {
                    value: 2,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    assert_eq!(old.await.unwrap(), Err(RefreshWaitError::Stale));
    assert_eq!(
        *service
            .request(&registration(2, false).identity, RefreshKind::Quota)
            .await
            .unwrap(),
        2
    );
    service.shutdown().await;
}

#[tokio::test]
async fn shutdown_cancels_workers_and_unblocks_waiters() {
    let service = RefreshService::<usize>::new(RefreshLimits::default()).unwrap();
    let (started, mut requests) = mpsc::unbounded_channel();
    service
        .register(registration(1, false), move |_| {
            let started = started.clone();
            Box::pin(async move {
                started.send(()).unwrap();
                std::future::pending().await
            })
        })
        .unwrap();
    let caller = service.clone();
    let task = tokio::spawn(async move {
        caller
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await
    });
    requests.recv().await.unwrap();
    service.shutdown().await;
    assert_eq!(task.await.unwrap(), Err(RefreshWaitError::Stopped));
    assert_eq!(
        service
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await,
        Err(RefreshWaitError::Stopped)
    );
}

#[tokio::test(start_paused = true)]
async fn unchanged_read_is_fresh_and_disabled_monitoring_does_not_retry_in_background() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    service
        .register(registration(1, false), move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                RefreshResult {
                    value: 0,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    service
        .request(&registration(1, false).identity, RefreshKind::Quota)
        .await
        .unwrap();
    assert_eq!(
        service.freshness(&registration(1, false).identity, RefreshKind::Quota),
        RefreshFreshness::Fresh { as_of_ms: 0 }
    );
    tokio::time::advance(Duration::from_secs(3_600)).await;
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.freshness(&registration(1, false).identity, RefreshKind::Quota),
        RefreshFreshness::Stale { as_of_ms: 0 }
    );
    service.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn background_manual_and_dirty_events_have_one_owner_and_one_follow_up() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let (started, mut requests) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let unblock = release.clone();
    let mut registration = registration(1, true);
    registration.due_now = true;
    let identity = registration.identity.clone();
    service
        .register(registration, move |job| {
            let (started, release) = (started.clone(), unblock.clone());
            Box::pin(async move {
                started.send(job.manual).unwrap();
                release.notified().await;
                RefreshResult {
                    value: 1,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    assert_eq!(requests.recv().await, Some(false));
    let caller = service.clone();
    let request_identity = identity.clone();
    let joined =
        tokio::spawn(async move { caller.request(&request_identity, RefreshKind::Quota).await });
    tokio::task::yield_now().await;
    for _ in 0..100 {
        assert!(service.mark_dirty(&identity, RefreshKind::Quota));
    }
    release.notify_one();
    assert_eq!(*joined.await.unwrap().unwrap(), 1);
    tokio::time::advance(Duration::from_millis(999)).await;
    tokio::task::yield_now().await;
    assert!(requests.try_recv().is_err());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(requests.recv().await, Some(false));
    release.notify_one();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert!(requests.try_recv().is_err());
    service.shutdown().await;
}

#[tokio::test]
async fn panicking_provider_releases_its_key_and_reports_a_safe_result() {
    let service = RefreshService::<usize>::new(RefreshLimits::default()).unwrap();
    service
        .register(registration(1, false), |_| {
            Box::pin(async { panic!("synthetic panic") })
        })
        .unwrap();
    assert_eq!(
        service
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await,
        Err(RefreshWaitError::Interrupted)
    );
    assert_eq!(service.state.lock().unwrap().coordinator.pending_jobs(), 0);
    service.shutdown().await;
}

#[tokio::test]
async fn a_late_old_registration_cannot_evict_a_newer_login_or_configuration() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let mut current = registration(2, false);
    current.identity.config_revision = 3;
    let identity = current.identity.clone();
    service
        .register(current, |_| {
            Box::pin(async {
                RefreshResult {
                    value: 2,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    for (auth, config) in [(1, 3), (2, 2), (3, 1)] {
        let mut stale = registration(auth, false);
        stale.identity.config_revision = config;
        assert_eq!(
            service.register(stale, |_| panic!("stale registration ran")),
            Err(RefreshWaitError::Stale)
        );
    }
    assert_eq!(
        *service
            .request(&identity, RefreshKind::Quota)
            .await
            .unwrap(),
        2
    );
    service.shutdown().await;
}

#[tokio::test]
async fn cached_reads_are_scoped_to_resource_and_revision() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let identity = registration(1, false).identity;
    for kind in [RefreshKind::Quota, RefreshKind::Balance] {
        let mut entry = registration(1, false);
        entry.kind = kind;
        service
            .register(entry, move |_| {
                Box::pin(async move {
                    RefreshResult {
                        value: kind,
                        outcome: RefreshOutcome::Success,
                    }
                })
            })
            .unwrap();
        assert_eq!(*service.request(&identity, kind).await.unwrap(), kind);
    }
    assert_eq!(
        *service.cached(&identity, RefreshKind::Balance).unwrap(),
        RefreshKind::Balance
    );
    assert!(service.remove_kind(&identity, RefreshKind::Balance));
    assert!(service.cached(&identity, RefreshKind::Balance).is_none());
    assert!(service.cached(&identity, RefreshKind::Quota).is_some());
    service
        .register(registration(2, false), |_| {
            Box::pin(async {
                RefreshResult {
                    value: RefreshKind::Quota,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    assert!(service.cached(&identity, RefreshKind::Quota).is_none());
    service.shutdown().await;
}

#[tokio::test]
async fn preparation_errors_reach_waiters_without_erasing_the_cached_observation() {
    let service =
        RefreshService::with_cache_policy(RefreshLimits::default(), Result::is_ok).unwrap();
    let identity = registration(1, false).identity;
    for value in [Ok(42_u64), Err("synthetic preparation failure")] {
        service
            .register(registration(1, false), move |_| {
                Box::pin(async move {
                    RefreshResult {
                        value,
                        outcome: if value.is_ok() {
                            RefreshOutcome::Success
                        } else {
                            RefreshOutcome::FailedRetryAt(60_000)
                        },
                    }
                })
            })
            .unwrap();
        assert_eq!(
            *service
                .request(&identity, RefreshKind::Quota)
                .await
                .unwrap(),
            value
        );
    }
    let (cached, freshness) = service
        .cached_observation(&identity, RefreshKind::Quota)
        .unwrap();
    assert_eq!(*cached, Ok(42));
    assert!(matches!(freshness, RefreshFreshness::Stale { .. }));
    service.shutdown().await;
}

#[tokio::test]
async fn quota_and_models_join_reserved_auth_without_caching_its_transient_result() {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Observation {
        Auth,
        Quota,
        Models,
    }

    let service = RefreshService::with_cache_policy(
        RefreshLimits {
            concurrent: 3,
            per_origin: 3,
            reserved_auth: 1,
            start_spacing_ms: 0,
            origin_spacing_ms: 0,
            minimum_interval_ms: 0,
            ..RefreshLimits::default()
        },
        |value: &Observation| *value != Observation::Auth,
    )
    .unwrap();
    let identity = registration(1, false).identity;
    let calls = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let (started, mut starts) = mpsc::unbounded_channel();
    let (count, unblock) = (calls.clone(), release.clone());
    let mut auth = registration(1, false);
    auth.kind = RefreshKind::Auth;
    service
        .register(auth, move |_| {
            let (count, started, release) = (count.clone(), started.clone(), unblock.clone());
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                started.send(()).unwrap();
                release.notified().await;
                RefreshResult {
                    value: Observation::Auth,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    for (kind, observation) in [
        (RefreshKind::Quota, Observation::Quota),
        (RefreshKind::Models, Observation::Models),
    ] {
        let mut entry = registration(1, false);
        entry.kind = kind;
        let weak = Arc::downgrade(&service);
        service
            .register(entry, move |_| {
                let weak = weak.clone();
                let identity = registration(1, false).identity;
                Box::pin(async move {
                    assert_eq!(
                        *weak
                            .upgrade()
                            .unwrap()
                            .request(&identity, RefreshKind::Auth)
                            .await
                            .unwrap(),
                        Observation::Auth
                    );
                    RefreshResult {
                        value: observation,
                        outcome: RefreshOutcome::Success,
                    }
                })
            })
            .unwrap();
    }
    let mut waiters = Vec::new();
    for (kind, observation) in [
        (RefreshKind::Quota, Observation::Quota),
        (RefreshKind::Models, Observation::Models),
    ] {
        let service = service.clone();
        let identity = identity.clone();
        waiters.push(tokio::spawn(async move {
            assert_eq!(
                *service.request(&identity, kind).await.unwrap(),
                observation
            );
        }));
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        starts.recv().await.unwrap();
        loop {
            let both_waiting = {
                let state = service.state.lock().unwrap();
                let auth_key = RefreshKey {
                    identity: identity.clone(),
                    kind: RefreshKind::Auth,
                };
                let auth_waiters = state.entries[&auth_key]
                    .result
                    .as_ref()
                    .unwrap()
                    .receiver_count();
                if auth_waiters == 2 {
                    assert_eq!(state.coordinator.pending_jobs(), 3);
                }
                auth_waiters == 2
            };
            if both_waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    release.notify_one();
    for waiter in waiters {
        waiter.await.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(service.cached(&identity, RefreshKind::Auth).is_none());
    assert_eq!(
        *service.cached(&identity, RefreshKind::Models).unwrap(),
        Observation::Models
    );
    service.shutdown().await;
}

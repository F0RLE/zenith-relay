use super::*;

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

use super::*;

#[tokio::test]
async fn replacement_releases_stale_waiters_without_publishing_stale_results() {
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
    let stale_service = service.clone();
    let stale_request = tokio::spawn(async move {
        stale_service
            .request(&registration(1, false).identity, RefreshKind::Quota)
            .await
    });
    requests.recv().await.unwrap();
    service
        .register(registration(2, false), |_| {
            Box::pin(async {
                RefreshResult {
                    refresh_value: 2,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    assert_eq!(stale_request.await.unwrap(), Err(RefreshWaitError::Stale));
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
                    refresh_value: 0,
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
                    refresh_value: 1,
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
async fn a_late_stale_registration_cannot_evict_a_newer_login_or_configuration() {
    let service = RefreshService::new(RefreshLimits::default()).unwrap();
    let mut current = registration(2, false);
    current.identity.config_revision = 3;
    let identity = current.identity.clone();
    service
        .register(current, |_| {
            Box::pin(async {
                RefreshResult {
                    refresh_value: 2,
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
        let mut refresh_registration = registration(1, false);
        refresh_registration.kind = kind;
        service
            .register(refresh_registration, move |_| {
                Box::pin(async move {
                    RefreshResult {
                        refresh_value: kind,
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
                    refresh_value: RefreshKind::Quota,
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
    for refresh_value in [Ok(42_u64), Err("synthetic preparation failure")] {
        service
            .register(registration(1, false), move |_| {
                Box::pin(async move {
                    RefreshResult {
                        refresh_value,
                        outcome: if refresh_value.is_ok() {
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
            refresh_value
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
        |observation: &Observation| *observation != Observation::Auth,
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
                    refresh_value: Observation::Auth,
                    outcome: RefreshOutcome::Success,
                }
            })
        })
        .unwrap();
    for (kind, observation) in [
        (RefreshKind::Quota, Observation::Quota),
        (RefreshKind::Models, Observation::Models),
    ] {
        let mut refresh_registration = registration(1, false);
        refresh_registration.kind = kind;
        let weak = Arc::downgrade(&service);
        service
            .register(refresh_registration, move |_| {
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
                        refresh_value: observation,
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
                    .completion_sender
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

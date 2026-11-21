use super::*;

#[tokio::test(start_paused = true)]
async fn repeated_full_capacity_waves_release_cancelled_waiters_without_losing_lanes() {
    let store = SocketStore::default();
    let keys: Vec<_> = (0..MAX_SESSIONS)
        .map(|index| planner::fingerprint(&json!(["retained", index])))
        .collect();
    let mut completed = 0;
    let mut cancelled = 0;
    for wave in 0..16 {
        let owners: Vec<_> = keys
            .iter()
            .map(|key| {
                let mut owner = store.acquire(*key).unwrap();
                owner.bind(authority(7));
                owner
            })
            .collect();
        let mut waiters = Vec::new();
        for index in 0..MAX_SESSIONS * 2 {
            let mut waiting = Box::pin(store.acquire_or_isolate(Some(keys[index % MAX_SESSIONS])));
            assert!(waiting.as_mut().now_or_never().is_none());
            if index % 3 == 0 {
                drop(waiting);
                cancelled += 1;
            } else {
                waiters.push(waiting);
            }
        }
        let fresh = planner::fingerprint(&json!(["fresh", wave]));
        let (isolated, wait_ms) = store.acquire_or_isolate(Some(fresh)).await;
        assert_eq!(wait_ms, 0);
        assert_eq!(
            isolated.reset_reason,
            Some(ConnectionResetReason::SessionCapacityExceeded)
        );
        drop(isolated);
        drop(owners);
        for waiting in waiters {
            let (owner, wait_ms) = waiting.await;
            assert_eq!(wait_ms, 0);
            assert!(owner.authority.as_ref() == Some(&authority(7)));
            completed += 1;
        }
        {
            let lanes = crate::poison::mutex(&store.lanes);
            assert_eq!(lanes.len(), MAX_SESSIONS);
            assert!(lanes.values().all(|lane| Arc::strong_count(lane) == 1));
        }
        let (replacement, wait_ms) = store.acquire_or_isolate(Some(fresh)).await;
        assert_eq!(wait_ms, 0);
        assert_eq!(
            replacement.reset_reason,
            Some(ConnectionResetReason::NewSession)
        );
    }
    assert_eq!(completed, 672);
    assert_eq!(cancelled, 352);
    assert_eq!(crate::poison::mutex(&store.lanes).len(), MAX_SESSIONS);
}

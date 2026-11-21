use super::*;
use crate::protocol::UsageWorkload;

mod load;

fn authority(marker: u8) -> SocketAuthority {
    SocketAuthority {
        credential: [marker; 32],
        incarnation: crate::runtime::AuthorizationIncarnation::Agent(1),
    }
}

#[tokio::test(start_paused = true)]
async fn short_overlap_reuses_the_retained_lane_after_its_owner_finishes() {
    let store = SocketStore::default();
    let key = [1; 32];
    let mut owner = store.acquire(key).unwrap();
    owner.bind(authority(7));
    let mut waiting = Box::pin(store.acquire_or_isolate(Some(key)));
    assert!(waiting.as_mut().now_or_never().is_none());
    tokio::time::advance(Duration::from_millis(75)).await;
    drop(owner);
    let (next, waited_ms) = waiting.await;
    assert_eq!(waited_ms, 75);
    assert!(next.authority.as_ref() == Some(&authority(7)));
    assert_eq!(crate::poison::mutex(&store.lanes).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelled_waiter_does_not_steal_or_discard_the_active_lane() {
    let store = SocketStore::default();
    let key = [2; 32];
    let mut owner = store.acquire(key).unwrap();
    owner.bind(authority(8));
    let mut waiting = Box::pin(store.acquire_or_isolate(Some(key)));
    assert!(waiting.as_mut().now_or_never().is_none());
    drop(waiting);
    assert_eq!(
        Arc::strong_count(&crate::poison::mutex(&store.lanes)[&key]),
        2
    );
    drop(owner);
    let (next, waited_ms) = store.acquire_or_isolate(Some(key)).await;
    assert_eq!(waited_ms, 0);
    assert!(next.authority.as_ref() == Some(&authority(8)));
}

#[tokio::test(start_paused = true)]
async fn queued_turns_retain_fifo_ownership_without_blocking_other_sessions() {
    let store = SocketStore::default();
    let key = [3; 32];
    let owner = store.acquire(key).unwrap();
    let mut first = Box::pin(store.acquire_or_isolate(Some(key)));
    let mut second = Box::pin(store.acquire_or_isolate(Some(key)));
    assert!(first.as_mut().now_or_never().is_none());
    assert!(second.as_mut().now_or_never().is_none());
    let (unrelated, waited_ms) = store.acquire_or_isolate(Some([4; 32])).await;
    assert_eq!(waited_ms, 0);
    drop(unrelated);
    drop(owner);
    let (mut first_owner, _) = first.await;
    first_owner.bind(authority(9));
    assert!(second.as_mut().now_or_never().is_none());
    drop(first_owner);
    let (second_owner, _) = second.await;
    assert!(second_owner.authority.as_ref() == Some(&authority(9)));
}

#[test]
fn pending_borrowers_pin_idle_lanes_against_expiry_and_capacity_eviction() {
    let store = SocketStore::default();
    let key = [5; 32];
    let mut owner = store.acquire(key).unwrap();
    owner.touched = Instant::now() - IDLE_TTL;
    let borrower = Arc::clone(&crate::poison::mutex(&store.lanes)[&key]);
    let owners: Vec<_> = (0..MAX_SESSIONS - 1)
        .map(|index| store.acquire(planner::fingerprint(&json!(index))).unwrap())
        .collect();
    drop(owner);
    let next = [6; 32];
    assert!(store.acquire(next).is_none());
    assert!(crate::poison::mutex(&store.lanes).contains_key(&key));
    drop(borrower);
    assert!(store.acquire(next).is_some());
    assert!(!crate::poison::mutex(&store.lanes).contains_key(&key));
    drop(owners);
}

#[tokio::test(start_paused = true)]
async fn memory_and_background_work_keep_their_own_reusable_lanes() {
    let store = SocketStore::default();
    let url = "https://api.openai.com/v1/responses".parse().unwrap();
    let prepared = planner::parse(&serde_json::to_vec(&request(json!([]))).unwrap()).unwrap();
    let key = |workload| {
        prepared
            .session_key("key", "account", "context", &url, workload)
            .unwrap()
    };
    let mut memory = store.acquire(key(UsageWorkload::Memory)).unwrap();
    memory.bind(authority(10));
    let (chat, waited_ms) = store
        .acquire_or_isolate(Some(key(UsageWorkload::Conversation)))
        .await;
    assert_eq!(waited_ms, 0);
    let (background, waited_ms) = store
        .acquire_or_isolate(Some(key(UsageWorkload::Background)))
        .await;
    assert_eq!(waited_ms, 0);
    drop(memory);
    let (next_memory, waited_ms) = store
        .acquire_or_isolate(Some(key(UsageWorkload::Memory)))
        .await;
    assert_eq!(waited_ms, 0);
    assert!(next_memory.authority.as_ref() == Some(&authority(10)));
    assert!(chat.authority.is_none());
    assert!(background.authority.is_none());
    assert_eq!(crate::poison::mutex(&store.lanes).len(), 3);
}

#[test]
fn workload_uses_pre_dispatch_origin_without_inventing_unknown_task_kinds() {
    for origin in ["memory_extraction", "memory_consolidation"] {
        assert_eq!(
            UsageWorkload::from_request_origin(Some(origin)),
            UsageWorkload::Memory
        );
    }
    for origin in ["activity_summary", "task_title", "wake"] {
        assert_eq!(
            UsageWorkload::from_request_origin(Some(origin)),
            UsageWorkload::Background
        );
    }
    for origin in [None, Some("synthetic-unknown")] {
        assert_eq!(
            UsageWorkload::from_request_origin(origin),
            UsageWorkload::Conversation
        );
    }
}

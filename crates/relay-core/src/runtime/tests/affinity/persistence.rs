use super::*;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::result::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

fn prompt_runtime(store: Arc<dyn ResponseAffinityStore>) -> Arc<GatewayRuntime> {
    Arc::new(
        GatewayRuntime::from_pool(
            vec![
                RuntimeSource::unrestricted(source("source-a", "secret-a", &["gpt-test"])),
                RuntimeSource::unrestricted(source("source-b", "secret-b", &["gpt-test"])),
            ],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions {
                response_affinity_store: Some(store),
                ..Default::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap(),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockedOperation {
    Find,
    Upsert,
}

struct BlockingPromptStore {
    bindings: Mutex<HashMap<String, ResponseAffinityBinding>>,
    blocked_key: String,
    operation: BlockedOperation,
    blocked_once: AtomicBool,
    entered: Sender<()>,
    release: Mutex<Receiver<()>>,
}

impl BlockingPromptStore {
    fn block_once(&self, key: &str, operation: BlockedOperation) -> Result<(), String> {
        if key != self.blocked_key
            || operation != self.operation
            || self.blocked_once.swap(true, Ordering::SeqCst)
        {
            return Ok(());
        }
        self.entered.send(()).map_err(|error| error.to_string())?;
        crate::poison::mutex(&self.release)
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| error.to_string())
    }
}

impl ResponseAffinityStore for BlockingPromptStore {
    fn load(&self, _now_ms: u64) -> Result<Vec<ResponseAffinityBinding>, String> {
        Ok(Vec::new())
    }

    fn find(&self, key: &str, _now_ms: u64) -> Result<Option<ResponseAffinityBinding>, String> {
        let binding = crate::poison::mutex(&self.bindings).get(key).cloned();
        self.block_once(key, BlockedOperation::Find)?;
        Ok(binding)
    }

    fn upsert(&self, binding: &ResponseAffinityBinding) -> Result<(), String> {
        self.block_once(&binding.key, BlockedOperation::Upsert)?;
        crate::poison::mutex(&self.bindings).insert(binding.key.clone(), binding.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        crate::poison::mutex(&self.bindings).remove(key);
        Ok(())
    }

    fn delete_candidate(&self, candidate_id: &str) -> Result<(), String> {
        crate::poison::mutex(&self.bindings)
            .retain(|_, binding| binding.candidate_id != candidate_id);
        Ok(())
    }
}

fn blocking_store(
    key: &str,
    operation: BlockedOperation,
) -> (Arc<BlockingPromptStore>, Receiver<()>, Sender<()>) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    (
        Arc::new(BlockingPromptStore {
            bindings: Mutex::new(HashMap::new()),
            blocked_key: key.to_owned(),
            operation,
            blocked_once: AtomicBool::new(false),
            entered: entered_tx,
            release: Mutex::new(release_rx),
        }),
        entered_rx,
        release_tx,
    )
}

#[test]
fn prompt_affinity_orders_durable_mutations_without_holding_the_scheduler() {
    let key = "cache:ordered-persistence";
    let (store, entered, release) = blocking_store(key, BlockedOperation::Upsert);
    let runtime = prompt_runtime(store.clone());
    let replacement = prompt_runtime(store.clone());
    let first_runtime = runtime.clone();
    let first = std::thread::spawn(move || {
        first_runtime.bind_prompt_affinity(Some(key), "source-a", 100);
    });
    let first_entered = entered.recv_timeout(Duration::from_secs(5));
    let scheduler_available = runtime.scheduler.try_lock().is_ok();

    let warm_runtime = runtime.clone();
    let (warm_done_tx, warm_done_rx) = mpsc::channel();
    let warm = std::thread::spawn(move || {
        warm_runtime.restore_prompt_affinity_from_store(key, 100);
        warm_done_tx.send(()).unwrap();
    });
    let warm_progressed = warm_done_rx.recv_timeout(Duration::from_secs(5));

    let next_runtime = runtime.clone();
    let next_replacement = replacement.clone();
    let (next_started_tx, next_started_rx) = mpsc::channel();
    let (next_done_tx, next_done_rx) = mpsc::channel();
    let next = std::thread::spawn(move || {
        next_started_tx.send(()).unwrap();
        let invalidated = next_runtime.invalidate_prompt_affinity(Some(key), "source-a");
        next_replacement.bind_prompt_affinity(Some(key), "source-b", 200);
        next_done_tx.send(()).unwrap();
        invalidated
    });
    let next_started = next_started_rx.recv_timeout(Duration::from_secs(5));
    let next_waited = next_done_rx
        .recv_timeout(Duration::from_millis(100))
        .is_err();

    // Match the production stripe hash to choose an independent chat without
    // relying on timing or assuming that arbitrary keys cannot collide.
    let stripe = |key: &str| {
        let mut hash = DefaultHasher::new();
        key.hash(&mut hash);
        (hash.finish() as usize) % 64
    };
    let other_key = (0..128)
        .map(|index| format!("cache:independent-{index}"))
        .find(|other| stripe(other) != stripe(key))
        .unwrap();
    let other_runtime = runtime.clone();
    let (other_done_tx, other_done_rx) = mpsc::channel();
    let other = std::thread::spawn(move || {
        other_runtime.bind_prompt_affinity(Some(&other_key), "source-a", 200);
        other_done_tx.send(()).unwrap();
    });
    let other_progressed = other_done_rx.recv_timeout(Duration::from_secs(5));

    // Release every worker before assertions, including on a failed invariant.
    let released = release.send(());
    first.join().unwrap();
    warm.join().unwrap();
    let invalidated = next.join().unwrap();
    other.join().unwrap();
    assert!(first_entered.is_ok());
    assert!(next_started.is_ok());
    assert!(released.is_ok());
    assert!(
        scheduler_available,
        "SQLite must not hold the scheduler lock"
    );
    assert!(
        next_waited,
        "same-key persistence must not overtake a write"
    );
    assert!(other_progressed.is_ok(), "independent chats must progress");
    assert!(
        warm_progressed.is_ok(),
        "warm selection must not wait for disk"
    );
    assert!(invalidated);
    assert_eq!(
        replacement
            .lock_scheduler()
            .prompt_affinity_snapshot(key, 200),
        Some(("source-b".to_owned(), 200 + crate::PROMPT_AFFINITY_TTL_MS))
    );
    let bindings = crate::poison::mutex(&store.bindings);
    let binding = bindings.get(key).unwrap();
    assert_eq!(binding.candidate_id, "source-b");
    assert_eq!(binding.expires_at_ms, 200 + crate::PROMPT_AFFINITY_TTL_MS);
}

#[test]
fn prompt_affinity_orders_lazy_restoration_before_a_successful_rebalance() {
    let key = "cache:ordered-restore";
    let (store, entered, release) = blocking_store(key, BlockedOperation::Find);
    let runtime = prompt_runtime(store.clone());
    for (candidate, quota) in [("source-a", 6_000), ("source-b", 9_000)] {
        assert!(runtime.update_candidate_availability_at(
            candidate,
            true,
            CandidateHealth::Healthy,
            CandidateQuota::Available(quota),
            Some(100),
        ));
    }
    // The completing request must have been admitted first. Admission projects
    // the current resource evidence into the rotation engine lazily.
    let completing = runtime
        .lock_scheduler()
        .select(crate::SelectionRequest {
            model: "gpt-test",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .unwrap();
    assert_eq!(completing.candidate_id, "source-b");
    crate::poison::mutex(&store.bindings).insert(
        key.to_owned(),
        ResponseAffinityBinding {
            key: key.to_owned(),
            candidate_id: "source-a".to_owned(),
            expires_at_ms: 100 + crate::PROMPT_AFFINITY_TTL_MS,
        },
    );
    let selecting_runtime = runtime.clone();
    let selecting = std::thread::spawn(move || {
        let authenticated = selecting_runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (_, lease) = selecting_runtime
                    .select_and_reserve(
                        &authenticated,
                        "gpt-test",
                        &[WireApi::Responses],
                        &HashSet::new(),
                        (None, Some(key)),
                        100,
                    )
                    .await
                    .unwrap();
                drop(lease);
            });
    });
    let find_entered = entered.recv_timeout(Duration::from_secs(5));
    let scheduler_available = runtime.scheduler.try_lock().is_ok();
    let binding_runtime = runtime.clone();
    let (bind_started_tx, bind_started_rx) = mpsc::channel();
    let (bind_done_tx, bind_done_rx) = mpsc::channel();
    let binding = std::thread::spawn(move || {
        bind_started_tx.send(()).unwrap();
        binding_runtime.bind_prompt_affinity(Some(key), "source-b", 200);
        bind_done_tx.send(()).unwrap();
    });
    let bind_started = bind_started_rx.recv_timeout(Duration::from_secs(5));
    let bind_waited = bind_done_rx
        .recv_timeout(Duration::from_millis(100))
        .is_err();
    let released = release.send(());
    selecting.join().unwrap();
    binding.join().unwrap();

    assert!(find_entered.is_ok());
    assert!(bind_started.is_ok());
    assert!(released.is_ok());
    assert!(scheduler_available);
    assert!(
        bind_waited,
        "restoration and writes must share key ordering"
    );
    assert_eq!(
        runtime.lock_scheduler().prompt_affinity_snapshot(key, 200),
        Some(("source-b".to_owned(), 200 + crate::PROMPT_AFFINITY_TTL_MS))
    );
    assert_eq!(
        crate::poison::mutex(&store.bindings)
            .get(key)
            .unwrap()
            .candidate_id,
        "source-b"
    );
}

#[test]
fn prompt_affinity_persists_the_monotonic_scheduler_expiry() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = prompt_runtime(store.clone());
    let key = "cache:monotonic-expiry";
    runtime.bind_prompt_affinity(Some(key), "source-a", 200);
    runtime.bind_prompt_affinity(Some(key), "source-a", 100);

    let expected = 200 + crate::PROMPT_AFFINITY_TTL_MS;
    assert_eq!(
        runtime.lock_scheduler().prompt_affinity_snapshot(key, 200),
        Some(("source-a".to_owned(), expected))
    );
    let upserts = crate::poison::mutex(&store.upserts);
    assert_eq!(upserts.len(), 2);
    assert!(upserts
        .iter()
        .all(|binding| binding.expires_at_ms == expected));
}

#[test]
fn prompt_affinity_late_restore_preserves_a_live_owner() {
    let runtime = prompt_runtime(Arc::new(RecordedResponseAffinityStore::default()));
    let key = "cache:live-owner";
    runtime.bind_prompt_affinity(Some(key), "source-b", 200);
    let mut scheduler = runtime.lock_scheduler();
    assert!(!scheduler.restore_prompt_affinity(
        key,
        "source-a",
        100 + crate::PROMPT_AFFINITY_TTL_MS,
        200,
    ));
    assert_eq!(
        scheduler.prompt_affinity_snapshot(key, 200),
        Some(("source-b".to_owned(), 200 + crate::PROMPT_AFFINITY_TTL_MS))
    );
}

#[test]
fn retired_runtime_cannot_change_prompt_affinity_persistence() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = prompt_runtime(store.clone());
    let key = "cache:retired-runtime";
    runtime.bind_prompt_affinity(Some(key), "source-a", 100);
    runtime.retire_for_replacement();
    let replacement = prompt_runtime(store.clone());
    replacement.bind_prompt_affinity(Some(key), "source-b", 200);

    runtime.bind_prompt_affinity(Some(key), "source-a", 300);
    assert!(!runtime.invalidate_prompt_affinity(Some(key), "source-a"));
    let mut scheduler = runtime.lock_scheduler();
    assert!(!scheduler.bind_prompt_affinity("cache:late-binding", "source-a", 300));
    assert!(!scheduler.restore_prompt_affinity(
        "cache:late-restore",
        "source-a",
        300 + crate::PROMPT_AFFINITY_TTL_MS,
        300,
    ));
    let upserts = crate::poison::mutex(&store.upserts);
    assert_eq!(upserts.len(), 2);
    assert_eq!(upserts[1].candidate_id, "source-b");
    assert_eq!(
        upserts[1].expires_at_ms,
        200 + crate::PROMPT_AFFINITY_TTL_MS
    );
    assert!(crate::poison::mutex(&store.deletes).is_empty());
}

#[test]
fn prompt_affinity_failure_only_invalidates_the_failing_owner() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = prompt_runtime(store.clone());
    let key = "cache:owner-scoped-rejection";
    runtime.bind_prompt_affinity(Some(key), "source-a", 100);
    assert!(!runtime.invalidate_prompt_affinity(Some(key), "source-b"));
    assert_eq!(
        runtime.lock_scheduler().prompt_affinity_snapshot(key, 100),
        Some(("source-a".to_owned(), 100 + crate::PROMPT_AFFINITY_TTL_MS))
    );
    assert!(crate::poison::mutex(&store.deletes).is_empty());

    assert!(runtime.invalidate_prompt_affinity(Some(key), "source-a"));
    runtime.bind_prompt_affinity(Some(key), "source-b", 200);
    assert!(!runtime.invalidate_prompt_affinity(Some(key), "source-a"));
    assert_eq!(
        runtime.lock_scheduler().prompt_affinity_snapshot(key, 200),
        Some(("source-b".to_owned(), 200 + crate::PROMPT_AFFINITY_TTL_MS))
    );
    assert_eq!(*crate::poison::mutex(&store.deletes), vec![key.to_owned()]);
    assert!(runtime.invalidate_prompt_affinity(Some(key), "source-b"));
    assert!(!runtime.lock_scheduler().has_prompt_affinity(key, 200));
}

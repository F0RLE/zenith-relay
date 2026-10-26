use super::*;
use crate::local_pool::state::DesktopState;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::mpsc,
    task::{Context, Waker},
    time::Duration,
};
use tokio::sync::oneshot;

struct Fixture {
    root: PathBuf,
    state: DesktopState,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("relay-snapshot-worker-{}", uuid::Uuid::new_v4()));
        let state = DesktopState::open(root.clone()).unwrap();
        Self { root, state }
    }

    async fn close(self) {
        drop(self.state);
        // Optional timing writers may still hold SQLite briefly.
        for attempt in 0..20 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(_) if attempt < 19 => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(error) => panic!("temporary worker cleanup failed: {error}"),
            }
        }
    }
}

fn assert_pending(future: Pin<&mut impl Future>) {
    assert!(
        future
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
}

#[tokio::test]
async fn projection_yields_the_async_thread_and_returns_its_result() {
    let fixture = Fixture::new();
    let async_thread = std::thread::current().id();
    let permit = fixture.state.runtime_snapshot_permit().await.unwrap();
    let (started, ready) = oneshot::channel();
    let (release, blocked) = mpsc::channel();
    let task = tokio::spawn(run(permit, move || {
        started.send(std::thread::current().id()).unwrap();
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(37)
    }));

    assert_ne!(ready.await.unwrap(), async_thread);
    // The single-thread async executor reaches this while projection is held.
    assert!(!task.is_finished());
    release.send(()).unwrap();
    assert_eq!(task.await.unwrap().unwrap(), 37);
    fixture.close().await;
}

#[tokio::test]
async fn clones_share_one_slot_without_blocking_mutations_or_other_states() {
    let fixture = Fixture::new();
    let independent = Fixture::new();
    let clone = fixture.state.clone();
    let first = fixture.state.runtime_snapshot_permit().await.unwrap();
    let mut waiting = Box::pin(clone.runtime_snapshot_permit());
    assert_pending(waiting.as_mut());

    let other = independent.state.runtime_snapshot_permit().await.unwrap();
    let mutation = fixture.state.setup_guard().await;
    drop(mutation);
    drop(other);
    drop(first);
    drop(waiting.await.unwrap());
    drop(clone);
    fixture.close().await;
    independent.close().await;
}

#[tokio::test]
async fn canceled_caller_keeps_the_slot_until_its_blocking_work_finishes() {
    let fixture = Fixture::new();
    let permit = fixture.state.runtime_snapshot_permit().await.unwrap();
    let (started, ready) = oneshot::channel();
    let (release, blocked) = mpsc::channel();
    let caller = tokio::spawn(run(permit, move || {
        started.send(()).unwrap();
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    }));
    ready.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    let mut waiting = Box::pin(fixture.state.runtime_snapshot_permit());
    assert_pending(waiting.as_mut());
    release.send(()).unwrap();
    drop(waiting.await.unwrap());
    fixture.close().await;
}

#[tokio::test]
async fn canceled_queued_reader_does_not_leak_a_slot() {
    let fixture = Fixture::new();
    let permit = fixture.state.runtime_snapshot_permit().await.unwrap();
    let mut waiting = Box::pin(fixture.state.runtime_snapshot_permit());
    assert_pending(waiting.as_mut());
    drop(waiting);
    drop(permit);
    drop(fixture.state.runtime_snapshot_permit().await.unwrap());
    fixture.close().await;
}

#[tokio::test]
async fn queued_snapshot_reads_fresh_inputs_and_preserves_projection() {
    let fixture = Fixture::new();
    let permit = fixture.state.runtime_snapshot_permit().await.unwrap();
    let mut queued = Box::pin(super::super::build_local_runtime_state(&fixture.state));
    assert_pending(queued.as_mut());
    let mut source = crate::local_pool::models::synthetic_responses_source();
    source.id = "synthetic-queued-source".into();
    source.secret_ref = format!("source:snapshot-worker-{}", uuid::Uuid::new_v4());
    fixture
        .state
        .store()
        .unwrap()
        .replace_records(vec![source], vec![])
        .unwrap();
    drop(permit);
    let snapshot = queued.await.unwrap();
    assert_eq!(snapshot.sources.len(), 1);
    assert_eq!(snapshot.sources[0].id, "synthetic-queued-source");

    let inputs = fixture.state.snapshot_inputs().await.unwrap();
    let runtime = fixture.state.gateway.runtime().await;
    let expected = super::super::project_local_runtime_state(
        &fixture.state,
        inputs,
        runtime,
        std::time::Instant::now(),
        super::super::timing::SnapshotTiming::new(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(snapshot).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    fixture.close().await;
}

#[tokio::test]
async fn projection_errors_are_preserved_and_panics_are_redacted() {
    let fixture = Fixture::new();
    let expected: CommandError =
        LocalPoolError::new(ErrorCode::InvalidState, "synthetic projection error").into();
    let failure = expected.clone();
    let permit = fixture.state.runtime_snapshot_permit().await.unwrap();
    let actual = run(permit, move || Err::<(), _>(failure))
        .await
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );

    let permit = fixture.state.runtime_snapshot_permit().await.unwrap();
    let error = run(permit, || -> Result<(), CommandError> {
        panic!("synthetic worker panic detail")
    })
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, "Runtime snapshot read was interrupted");
    drop(fixture.state.runtime_snapshot_permit().await.unwrap());
    fixture.close().await;
}

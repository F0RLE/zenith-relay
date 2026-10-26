//! Preserve the synchronous usage acknowledgement without running disk I/O on
//! Tokio's request workers. The queue is bounded; a full queue applies pressure
//! rather than dropping accounting, and the last owner drains/joins the writer.
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use zenith_relay_core::UsageCallback;

const QUEUE_CAPACITY: usize = 32;

struct AcknowledgedWriter<T> {
    sender: Option<mpsc::SyncSender<WriteJob<T>>>,
    thread: Option<JoinHandle<()>>,
}

struct WriteJob<T> {
    event: T,
    runtime: Option<tokio::runtime::Handle>,
    acknowledgement: mpsc::SyncSender<bool>,
}

impl<T: Send + 'static> AcknowledgedWriter<T> {
    fn start(write: impl Fn(T) + Send + 'static) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<WriteJob<T>>(QUEUE_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("relay-usage-writer".into())
            .spawn(move || {
                while let Ok(WriteJob {
                    event,
                    runtime,
                    acknowledgement,
                }) = receiver.recv()
                {
                    // Refresh notifications may spawn onto the caller's runtime.
                    let _runtime = runtime.as_ref().map(tokio::runtime::Handle::enter);
                    // An uncertain partial write must never be replayed. Report the
                    // failure, unblock its caller, and keep processing later events.
                    let completed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        write(event);
                    }))
                    .is_ok();
                    let _ = acknowledgement.send(completed);
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
        })
    }

    fn record(&self, event: T) -> bool {
        let runtime = tokio::runtime::Handle::try_current().ok();
        let can_handoff = runtime.as_ref().is_some_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        });
        let write = || {
            let (acknowledgement, done) = mpsc::sync_channel(1);
            self.sender.as_ref().is_some_and(|sender| {
                sender
                    .send(WriteJob {
                        event,
                        runtime,
                        acknowledgement,
                    })
                    .is_ok()
            }) && done.recv().unwrap_or(false)
        };
        if can_handoff {
            // The callback API is deliberately synchronous: completion still means
            // the existing persistence/state callback finished, not just enqueue.
            tokio::task::block_in_place(write)
        } else {
            // Synchronous callers and current-thread test runtimes cannot hand off
            // their executor. Retain the original blocking acknowledgement contract.
            write()
        }
    }
}

impl<T> Drop for AcknowledgedWriter<T> {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(in crate::local_pool) fn offload(
    callback: super::TimedUsageCallback,
    failed: Arc<std::sync::atomic::AtomicU64>,
) -> UsageCallback {
    let write = callback.clone();
    let worker =
        match AcknowledgedWriter::start(move |(event, observed_at)| write(event, observed_at)) {
            Ok(worker) => worker,
            Err(_) => {
                crate::diagnostics::record_error(
                    "usage-writer",
                    Some("usage_writer_start_failed"),
                    "Background usage writer unavailable; retaining synchronous persistence",
                    &[],
                );
                return Arc::new(move |event| callback(event, chrono::Utc::now()));
            }
        };
    Arc::new(move |event| {
        if !worker.record((event, chrono::Utc::now())) {
            failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    #[test]
    fn acknowledgements_follow_writes_and_preserve_order() {
        let values = Arc::new(Mutex::new(Vec::new()));
        let target = values.clone();
        let writer =
            AcknowledgedWriter::start(move |value| target.lock().unwrap().push(value)).unwrap();
        for value in 0..64 {
            assert!(writer.record(value));
            assert_eq!(values.lock().unwrap().len(), value + 1);
        }
        drop(writer);
        assert_eq!(*values.lock().unwrap(), (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn full_queue_preserves_pending_writes_and_acknowledges_after_processing() {
        let (started, start) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let values = Arc::new(Mutex::new(Vec::new()));
        let target = values.clone();
        let writer = AcknowledgedWriter::start(move |value| {
            if value == 0 {
                started.send(()).unwrap();
                blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            target.lock().unwrap().push(value);
        })
        .unwrap();
        let sender = writer.sender.as_ref().unwrap();
        let mut acknowledgements = Vec::new();
        for event in 0..=QUEUE_CAPACITY {
            let (acknowledgement, done) = mpsc::sync_channel(1);
            assert!(
                sender
                    .try_send(WriteJob {
                        event,
                        runtime: None,
                        acknowledgement,
                    })
                    .is_ok()
            );
            acknowledgements.push(done);
            if event == 0 {
                start.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
        let (acknowledgement, _) = mpsc::sync_channel(1);
        assert!(matches!(
            sender.try_send(WriteJob {
                event: QUEUE_CAPACITY + 1,
                runtime: None,
                acknowledgement,
            }),
            Err(mpsc::TrySendError::Full(_))
        ));
        assert!(
            acknowledgements
                .iter()
                .all(|done| matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)))
        );
        release.send(()).unwrap();
        assert!(writer.record(QUEUE_CAPACITY + 1));
        drop(writer);
        for done in acknowledgements {
            assert!(done.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        assert_eq!(
            *values.lock().unwrap(),
            (0..=QUEUE_CAPACITY + 1).collect::<Vec<_>>()
        );
    }

    #[test]
    fn failed_write_is_not_replayed_and_next_write_still_completes() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let target = calls.clone();
        let writer = AcknowledgedWriter::start(move |value| {
            target.lock().unwrap().push(value);
            assert_ne!(value, 1, "synthetic writer failure");
        })
        .unwrap();
        assert!(!writer.record(1));
        assert!(writer.record(2));
        assert_eq!(*calls.lock().unwrap(), vec![1, 2]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn slow_persistence_does_not_stall_the_request_executor() {
        let timer_ran = Arc::new(AtomicBool::new(false));
        let flag = timer_ran.clone();
        let writer = AcknowledgedWriter::start(move |()| {
            // This write waits for another task on the single-worker runtime.
            let started = std::time::Instant::now();
            while !flag.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(flag.load(Ordering::Acquire));
        })
        .unwrap();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            timer_ran.store(true, Ordering::Release);
        });
        let completed = tokio::spawn(async move { writer.record(()) })
            .await
            .unwrap();
        timer.await.unwrap();
        assert!(completed);
    }
}

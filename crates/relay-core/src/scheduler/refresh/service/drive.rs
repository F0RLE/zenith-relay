use super::super::RefreshCompletion;
use super::*;
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::{atomic::Ordering, Arc, Weak};
use std::time::Duration;
use tokio::{sync::watch, task::JoinSet};

impl<T: Send + Sync + 'static> RefreshService<T> {
    pub(super) fn remove_entry(state: &mut State<T>, key: &RefreshKey) {
        state.coordinator.remove_kind(&key.identity, key.kind);
        if let Some(entry) = state.entries.remove(key) {
            if let Some(result) = entry.result {
                result.send_replace(Some(Err(RefreshWaitError::Stale)));
            }
        }
    }

    pub(super) fn start(self: &Arc<Self>) {
        if !self.started.swap(true, Ordering::AcqRel) {
            tokio::spawn(Self::drive(
                Arc::downgrade(self),
                self.changed.subscribe(),
                self.finished.clone(),
            ));
        }
    }

    pub(super) fn signal(&self) {
        self.changed
            .send_modify(|value| *value = value.wrapping_add(1));
    }

    async fn drive(
        service: Weak<Self>,
        mut changed: watch::Receiver<u64>,
        finished: watch::Sender<bool>,
    ) {
        let mut jobs = JoinSet::new();
        loop {
            // Subscribe/mark observed before checking due state. Changes between
            // this check and select remain visible (no lost release wakeup).
            changed.borrow_and_update();
            let delay = {
                let Some(service) = service.upgrade() else {
                    break;
                };
                let mut state = service.state.lock().expect("refresh state poisoned");
                if state.stopped {
                    break;
                }
                let now = service.now_ms();
                let mut started = false;
                while let Some(job) = state.coordinator.claim_due(now) {
                    started = true;
                    let key = RefreshKey {
                        identity: job.identity.clone(),
                        kind: job.kind,
                    };
                    let entry = state.entries.get_mut(&key).expect("registered work");
                    entry.result.get_or_insert_with(|| watch::channel(None).0);
                    let work = entry.work.clone();
                    jobs.spawn(async move {
                        let result = AssertUnwindSafe(async { work(job.clone()).await })
                            .catch_unwind()
                            .await;
                        (job, result.map_err(|_| RefreshWaitError::Interrupted))
                    });
                }
                if started {
                    service.notify_progress();
                }
                state
                    .coordinator
                    .next_wake()
                    .map(|at| Duration::from_millis(at.saturating_sub(now)))
            };
            tokio::select! {
                change = changed.changed() => { if change.is_err() { break; } }
                completion = jobs.join_next(), if !jobs.is_empty() => {
                    if let Some(Ok((job, result))) = completion {
                        let Some(service) = service.upgrade() else { break; };
                        service.complete(job, result);
                    }
                }
                _ = async {
                    match delay { Some(delay) => tokio::time::sleep(delay.min(Duration::from_secs(86_400))).await,
                        None => std::future::pending().await }
                } => {}
            }
        }
        jobs.shutdown().await;
        finished.send_replace(true);
    }

    fn complete(&self, job: RefreshJob, result: Result<RefreshResult<T>, RefreshWaitError>) {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let now = self.now_ms();
        let outcome = result
            .as_ref()
            .map_or(RefreshOutcome::retry_after(now, None), |result| {
                result.outcome
            });
        if !matches!(
            state.coordinator.complete(&job, outcome, now),
            RefreshCompletion::Applied { .. }
        ) {
            return;
        }
        let key = RefreshKey {
            identity: job.identity,
            kind: job.kind,
        };
        if let Some(entry) = state.entries.get_mut(&key) {
            let result = result.map(|result| Arc::new(result.value));
            if let Ok(value) = &result {
                if (self.cache_value)(value) {
                    entry.cached = Some(value.clone());
                }
            }
            if let Some(sender) = entry.result.take() {
                sender.send_replace(Some(result));
            }
        }
        drop(state);
        self.notify_progress();
    }
}

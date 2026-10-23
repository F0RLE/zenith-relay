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
        if let Some(refresh_entry) = state.entries.remove(key) {
            if let Some(completion_sender) = refresh_entry.completion_sender {
                completion_sender.send_replace(Some(Err(RefreshWaitError::Stale)));
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
            .send_modify(|revision| *revision = revision.wrapping_add(1));
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
                let mut service_state = service.state.lock().expect("refresh state poisoned");
                if service_state.stopped {
                    break;
                }
                let now = service.now_ms();
                let mut job_started = false;
                while let Some(job) = service_state.coordinator.claim_due(now) {
                    job_started = true;
                    let key = RefreshKey {
                        identity: job.identity.clone(),
                        kind: job.kind,
                    };
                    let refresh_entry = service_state
                        .entries
                        .get_mut(&key)
                        .expect("registered work");
                    refresh_entry
                        .completion_sender
                        .get_or_insert_with(|| watch::channel(None).0);
                    let work = refresh_entry.work.clone();
                    jobs.spawn(async move {
                        let job_result = AssertUnwindSafe(async { work(job.clone()).await })
                            .catch_unwind()
                            .await;
                        (job, job_result.map_err(|_| RefreshWaitError::Interrupted))
                    });
                }
                if job_started {
                    service.notify_progress();
                }
                service_state
                    .coordinator
                    .next_wake()
                    .map(|at| Duration::from_millis(at.saturating_sub(now)))
            };
            tokio::select! {
                change = changed.changed() => { if change.is_err() { break; } }
                completion = jobs.join_next(), if !jobs.is_empty() => {
                    if let Some(Ok((job, job_result))) = completion {
                        let Some(service) = service.upgrade() else { break; };
                        service.complete(job, job_result);
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

    fn complete(&self, job: RefreshJob, job_result: Result<RefreshResult<T>, RefreshWaitError>) {
        let mut service_state = self.state.lock().expect("refresh state poisoned");
        let now = self.now_ms();
        let outcome = job_result
            .as_ref()
            .map_or(RefreshOutcome::retry_after(now, None), |refresh_result| {
                refresh_result.outcome
            });
        if !matches!(
            service_state.coordinator.complete(&job, outcome, now),
            RefreshCompletion::Applied { .. }
        ) {
            return;
        }
        let key = RefreshKey {
            identity: job.identity,
            kind: job.kind,
        };
        if let Some(refresh_entry) = service_state.entries.get_mut(&key) {
            let observation =
                job_result.map(|refresh_result| Arc::new(refresh_result.refresh_value));
            if let Ok(cached_value) = &observation {
                if (self.is_cacheable)(cached_value) {
                    refresh_entry.cached = Some(cached_value.clone());
                }
            }
            if let Some(completion_sender) = refresh_entry.completion_sender.take() {
                completion_sender.send_replace(Some(observation));
            }
        }
        drop(service_state);
        self.notify_progress();
    }
}

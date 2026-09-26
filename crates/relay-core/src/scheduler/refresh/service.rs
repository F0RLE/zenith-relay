//! Shared asynchronous lifecycle for the deterministic refresh coordinator.
//! Hosts register provider-owned reads, not parallel timer/single-flight loops.
//! A caller owns only a subscription; cancellation never cancels another caller's read.

use super::{
    RefreshCompletion, RefreshCoordinator, RefreshFreshness, RefreshIdentity, RefreshJob,
    RefreshKey, RefreshKind, RefreshLimits, RefreshOutcome,
};
use futures_util::{future::BoxFuture, FutureExt};
use std::{
    collections::BTreeMap,
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use tokio::{sync::watch, task::JoinSet, time::Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshWaitError {
    Full,
    Stale,
    Stopped,
    Interrupted,
}

pub struct RefreshRegistration {
    pub identity: RefreshIdentity,
    pub kind: RefreshKind,
    /// Normalized provider origin, never an authenticated URL or proxy secret.
    pub origin: String,
    pub active: bool,
    pub automatic: bool,
    pub due_now: bool,
}

pub struct RefreshResult<T> {
    pub value: T,
    pub outcome: RefreshOutcome,
}

type SharedResult<T> = Option<Result<Arc<T>, RefreshWaitError>>;
type Work<T> = Arc<dyn Fn(RefreshJob) -> BoxFuture<'static, RefreshResult<T>> + Send + Sync>;

struct Entry<T> {
    work: Work<T>,
    result: Option<watch::Sender<SharedResult<T>>>,
    cached: Option<Arc<T>>,
}

struct State<T> {
    coordinator: RefreshCoordinator,
    entries: BTreeMap<RefreshKey, Entry<T>>,
    stopped: bool,
}

pub struct RefreshService<T> {
    state: Mutex<State<T>>,
    clock: Instant,
    changed: watch::Sender<u64>,
    started: AtomicBool,
    finished: watch::Sender<bool>,
    progress: watch::Sender<u64>,
    cache_value: fn(&T) -> bool,
}

impl<T: Send + Sync + 'static> RefreshService<T> {
    pub fn new(limits: RefreshLimits) -> Result<Arc<Self>, &'static str> {
        Self::with_cache_policy(limits, |_| true)
    }

    /// Preparation failures can reach waiters without erasing a previous
    /// observation. Hosts decide which typed results contain observations.
    pub fn with_cache_policy(
        limits: RefreshLimits,
        cache_value: fn(&T) -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                coordinator: RefreshCoordinator::new(limits)?,
                entries: BTreeMap::new(),
                stopped: false,
            }),
            clock: Instant::now(),
            changed: watch::channel(0).0,
            started: AtomicBool::new(false),
            finished: watch::channel(false).0,
            progress: watch::channel(0).0,
            cache_value,
        }))
    }

    /// Coalesced start/completion notifications. Observers read state only
    /// after transitions have committed; watching does not mark members active.
    pub fn progress(&self) -> watch::Receiver<u64> {
        self.progress.subscribe()
    }

    fn notify_progress(&self) {
        self.progress
            .send_modify(|value| *value = value.wrapping_add(1));
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }

    pub fn register<F>(
        self: &Arc<Self>,
        registration: RefreshRegistration,
        work: F,
    ) -> Result<(), RefreshWaitError>
    where
        F: Fn(RefreshJob) -> BoxFuture<'static, RefreshResult<T>> + Send + Sync + 'static,
    {
        let mut state = self.state.lock().expect("refresh state poisoned");
        if state.stopped {
            return Err(RefreshWaitError::Stopped);
        }
        let key = RefreshKey {
            identity: registration.identity,
            kind: registration.kind,
        };
        // A caller may have captured its snapshot before a concurrent edit.
        // Its late registration must not evict an already installed revision.
        if state
            .entries
            .range(RefreshKey::member_range(&key.identity.member_id))
            .any(|(current, _)| {
                current.kind == key.kind
                    && current.identity.member_id == key.identity.member_id
                    && (current.identity.auth_revision > key.identity.auth_revision
                        || current.identity.config_revision > key.identity.config_revision)
            })
        {
            return Err(RefreshWaitError::Stale);
        }
        // Configuration reconciliation replaces obsolete queued revisions, but
        // their running HTTP still charges capacity until it actually finishes.
        let obsolete = state
            .entries
            .range(RefreshKey::member_range(&key.identity.member_id))
            .map(|(key, _)| key)
            .filter(|old| {
                old.kind == key.kind
                    && old.identity.member_id == key.identity.member_id
                    && *old != &key
            })
            .cloned()
            .collect::<Vec<_>>();
        for old in obsolete {
            Self::remove_entry(&mut state, &old);
        }
        let new = !state.entries.contains_key(&key);
        state
            .coordinator
            .set_active(&key.identity, registration.active, self.now_ms());
        if !state.coordinator.register_origin(
            key.identity.clone(),
            key.kind,
            registration.origin,
            self.now_ms(),
            registration.active,
            new && registration.due_now && registration.automatic,
        ) {
            return Err(RefreshWaitError::Full);
        }
        // Do not clear an already pending manual request during reconciliation.
        let pending = state
            .entries
            .get(&key)
            .is_some_and(|entry| entry.result.is_some());
        let activated =
            state
                .coordinator
                .set_automatic(&key.identity, key.kind, registration.automatic);
        if activated && registration.due_now {
            state
                .coordinator
                .schedule_event(&key.identity, key.kind, self.now_ms());
        }
        if pending {
            state
                .coordinator
                .request_now(&key.identity, key.kind, self.now_ms());
        }
        let work: Work<T> = Arc::new(work);
        state
            .entries
            .entry(key)
            .and_modify(|entry| entry.work = work.clone())
            .or_insert(Entry {
                work,
                result: None,
                cached: None,
            });
        drop(state);
        self.start();
        self.signal();
        Ok(())
    }

    pub async fn request(
        self: &Arc<Self>,
        identity: &RefreshIdentity,
        kind: RefreshKind,
    ) -> Result<Arc<T>, RefreshWaitError> {
        let mut receiver = {
            let mut state = self.state.lock().expect("refresh state poisoned");
            if state.stopped {
                return Err(RefreshWaitError::Stopped);
            }
            if !state.coordinator.request_now(identity, kind, self.now_ms()) {
                return Err(RefreshWaitError::Stale);
            }
            let key = RefreshKey {
                identity: identity.clone(),
                kind,
            };
            let entry = state.entries.get_mut(&key).ok_or(RefreshWaitError::Stale)?;
            entry
                .result
                .get_or_insert_with(|| watch::channel(None).0)
                .subscribe()
        };
        self.start();
        self.signal();
        loop {
            if let Some(result) = receiver.borrow_and_update().as_ref() {
                return result.clone();
            }
            receiver
                .changed()
                .await
                .map_err(|_| RefreshWaitError::Interrupted)?;
        }
    }

    pub fn mark_dirty(&self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        let changed = self
            .state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .mark_dirty(identity, kind, self.now_ms());
        if changed {
            self.signal();
        }
        changed
    }

    /// Host calls this only after a validated passive quota snapshot was
    /// persisted. The age is measured from the oldest supplied quota window.
    pub fn observe_passive_quota(
        &self,
        identity: &RefreshIdentity,
        age_ms: u64,
        observed_at_wall_ms: u64,
    ) -> bool {
        let observed = self
            .state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .observe_passive_quota(identity, self.now_ms(), age_ms, observed_at_wall_ms);
        if observed {
            self.signal();
        }
        observed
    }

    pub fn set_active(&self, identity: &RefreshIdentity, active: bool) {
        let changed = self
            .state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .set_active(identity, active, self.now_ms());
        if changed {
            self.signal();
        }
    }

    /// A host event may accelerate a registered automatic job, never bypass a
    /// provider floor or revive an unsupported/disabled resource kind.
    pub fn schedule_after(
        &self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        delay_ms: u64,
    ) -> bool {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let scheduled = state.coordinator.schedule_event(
            identity,
            kind,
            self.now_ms().saturating_add(delay_ms),
        );
        drop(state);
        self.signal();
        scheduled
    }

    pub fn in_flight(&self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        self.state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .in_flight(identity, kind)
    }

    pub fn remove_member(&self, member_id: &str) -> bool {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let keys = state
            .entries
            .range(RefreshKey::member_range(member_id))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in &keys {
            Self::remove_entry(&mut state, key);
        }
        drop(state);
        self.signal();
        if !keys.is_empty() {
            self.notify_progress();
        }
        !keys.is_empty()
    }

    /// Retire one resource after its own endpoint changes. A different kind's
    /// in-flight read still charges capacity until it actually finishes.
    pub fn remove_kind(&self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let key = RefreshKey {
            identity: identity.clone(),
            kind,
        };
        if !state.entries.contains_key(&key) {
            return false;
        }
        Self::remove_entry(&mut state, &key);
        drop(state);
        self.signal();
        self.notify_progress();
        true
    }

    pub fn respect_retry_after(
        &self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        delay_ms: u64,
    ) {
        self.state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .defer_until(identity, kind, self.now_ms().saturating_add(delay_ms));
        self.signal();
    }

    pub fn set_member_active(&self, member_id: &str) {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let identities = state
            .entries
            .range(RefreshKey::member_range(member_id))
            .map(|(key, _)| key)
            .map(|key| key.identity.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let mut changed = false;
        for identity in identities {
            changed |= state.coordinator.set_active(&identity, true, self.now_ms());
        }
        drop(state);
        if changed {
            self.signal();
        }
    }

    pub fn retain(&self, mut keep: impl FnMut(&RefreshIdentity, RefreshKind) -> bool) {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let removed = state
            .entries
            .keys()
            .filter(|key| !keep(&key.identity, key.kind))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed {
            Self::remove_entry(&mut state, &key);
        }
        drop(state);
        self.signal();
    }

    pub fn cached(&self, identity: &RefreshIdentity, kind: RefreshKind) -> Option<Arc<T>> {
        self.cached_observation(identity, kind)
            .map(|(value, _)| value)
    }

    /// Cache and freshness are one observation, not two independently racing reads.
    pub fn cached_observation(
        &self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
    ) -> Option<(Arc<T>, RefreshFreshness)> {
        let state = self.state.lock().expect("refresh state poisoned");
        let value = state
            .entries
            .get(&RefreshKey {
                identity: identity.clone(),
                kind,
            })
            .and_then(|entry| entry.cached.clone())?;
        Some((
            value,
            state.coordinator.freshness(identity, kind, self.now_ms()),
        ))
    }

    pub fn freshness(&self, identity: &RefreshIdentity, kind: RefreshKind) -> RefreshFreshness {
        self.state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .freshness(identity, kind, self.now_ms())
    }

    pub async fn shutdown(&self) {
        {
            let mut state = self.state.lock().expect("refresh state poisoned");
            state.stopped = true;
            for (_, entry) in std::mem::take(&mut state.entries) {
                if let Some(result) = entry.result {
                    result.send_replace(Some(Err(RefreshWaitError::Stopped)));
                }
            }
        }
        let mut finished = self.finished.subscribe();
        self.signal();
        if !self.started.load(Ordering::Acquire) {
            return;
        }
        loop {
            if *finished.borrow_and_update() {
                return;
            }
            if finished.changed().await.is_err() {
                return;
            }
        }
    }

    fn remove_entry(state: &mut State<T>, key: &RefreshKey) {
        state.coordinator.remove_kind(&key.identity, key.kind);
        if let Some(entry) = state.entries.remove(key) {
            if let Some(result) = entry.result {
                result.send_replace(Some(Err(RefreshWaitError::Stale)));
            }
        }
    }

    fn start(self: &Arc<Self>) {
        if !self.started.swap(true, Ordering::AcqRel) {
            tokio::spawn(Self::drive(
                Arc::downgrade(self),
                self.changed.subscribe(),
                self.finished.clone(),
            ));
        }
    }

    fn signal(&self) {
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
        let outcome = result.as_ref().map_or(
            RefreshOutcome::FailedRetryAt(now.saturating_add(60_000)),
            |result| result.outcome,
        );
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

#[cfg(test)]
mod tests;

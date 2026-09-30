//! Shared asynchronous lifecycle for the deterministic refresh coordinator.
//! Hosts register provider-owned reads, not parallel timer/single-flight loops.
//! A caller owns only a subscription; cancellation never cancels another caller's read.

use super::{
    RefreshCoordinator, RefreshFreshness, RefreshIdentity, RefreshJob, RefreshKey, RefreshKind,
    RefreshLimits, RefreshOutcome,
};
use futures_util::future::BoxFuture;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{sync::watch, time::Instant};

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
}

impl<T: Send + Sync + 'static> RefreshService<T> {
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
}

mod membership;
mod register;

mod drive;
#[cfg(test)]
mod tests;

use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct StoreState {
    scopes: BTreeMap<Fingerprint, ScopeState>,
    pub(super) active: BTreeMap<Fingerprint, ActiveRequest>,
    sequence: u64,
}

#[derive(Default)]
struct ScopeState {
    previous: Option<Arc<CompletedRequest>>,
    active: BTreeSet<Fingerprint>,
    last_touched_ms: u64,
}

pub(super) struct ActiveRequest {
    observed: Weak<Observation>,
    pub(super) sequence: u64,
    pub(super) prepared: Option<PreparedRequest>,
}

pub(super) struct PreparedRequest {
    pub(super) candidate: Fingerprint,
    pub(super) upstream: Option<Arc<RequestFingerprint>>,
}

pub(super) struct CompletedRequest {
    pub(super) client: Arc<RequestFingerprint>,
    pub(super) upstream: Arc<RequestFingerprint>,
    pub(super) candidate: Fingerprint,
    pub(super) completed_at_ms: u64,
    sequence: u64,
}

impl StoreState {
    #[cfg(test)]
    pub(super) fn scope_count(&self) -> usize {
        self.scopes.len()
    }

    pub(super) fn prune(&mut self, now_ms: u64) {
        self.active
            .retain(|_, active| active.observed.strong_count() > 0);
        for scope in self.scopes.values_mut() {
            scope.active.retain(|key| self.active.contains_key(key));
        }
        self.scopes.retain(|_, scope| {
            if scope.previous.as_ref().is_some_and(|previous| {
                now_ms.saturating_sub(previous.completed_at_ms) >= BASELINE_TTL_MS
            }) {
                scope.previous = None;
            }
            !scope.active.is_empty()
                || now_ms.saturating_sub(scope.last_touched_ms) < BASELINE_TTL_MS
        });
    }

    pub(super) fn reserve(
        &mut self,
        scope_key: Option<Fingerprint>,
        request_key: Fingerprint,
        now_ms: u64,
    ) -> (u64, Option<Arc<CompletedRequest>>, bool) {
        self.sequence = self.sequence.saturating_add(1);
        let Some(scope_key) = scope_key else {
            return (self.sequence, None, false);
        };
        // Even a request beyond the diagnostic capacity invalidates comparisons
        // for observations already running in its scope.
        if let Some(scope) = self.scopes.get(&scope_key) {
            for active_key in &scope.active {
                if let Some(observed) = self
                    .active
                    .get(active_key)
                    .and_then(|active| active.observed.upgrade())
                {
                    observed.overlapping.store(true, Ordering::Relaxed);
                }
            }
        }
        if self.active.len() >= MAX_ACTIVE_REQUESTS || self.active.contains_key(&request_key) {
            return (self.sequence, None, false);
        }
        if self.scopes.len() >= MAX_SCOPES && !self.scopes.contains_key(&scope_key) {
            let oldest = self
                .scopes
                .iter()
                .filter(|(_, scope)| scope.active.is_empty())
                .min_by_key(|(_, scope)| scope.last_touched_ms)
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                self.scopes.remove(&oldest);
            } else {
                return (self.sequence, None, false);
            }
        }
        let scope = self.scopes.entry(scope_key).or_default();
        scope.last_touched_ms = now_ms;
        (self.sequence, scope.previous.clone(), true)
    }

    pub(super) fn track(&mut self, observed: &Arc<Observation>, now_ms: u64) {
        let Some(scope) = observed.scope_key.and_then(|key| self.scopes.get_mut(&key)) else {
            return;
        };
        observed
            .overlapping
            .store(!scope.active.is_empty(), Ordering::Relaxed);
        scope.active.insert(observed.request_key);
        scope.last_touched_ms = now_ms;
        self.active.insert(
            observed.request_key,
            ActiveRequest {
                observed: Arc::downgrade(observed),
                sequence: observed.sequence,
                prepared: None,
            },
        );
    }

    pub(super) fn remove_active(&mut self, request_key: &Fingerprint, sequence: u64) {
        if self
            .active
            .get(request_key)
            .is_none_or(|active| active.sequence != sequence)
        {
            return;
        }
        self.active.remove(request_key);
        for scope in self.scopes.values_mut() {
            scope.active.remove(request_key);
        }
    }

    pub(super) fn finish(
        &mut self,
        request_key: Fingerprint,
        candidate: Fingerprint,
        event: &mut UsageEvent,
        now_ms: u64,
    ) {
        let Some(active) = self.active.get_mut(&request_key) else {
            return;
        };
        if active
            .prepared
            .as_ref()
            .is_none_or(|prepared| prepared.candidate != candidate)
        {
            return;
        }
        let Some(observed) = active.observed.upgrade() else {
            return;
        };
        let Some(diagnostics) = event
            .routing
            .as_mut()
            .and_then(|routing| routing.cache_context.as_mut())
        else {
            return;
        };
        let Some(prepared) = active.prepared.take() else {
            return;
        };
        // Another request can begin after this payload was prepared. Correct the
        // persisted classification at completion rather than falsely blaming an
        // overlapping client for a cache-prefix rewrite.
        if observed.overlapping.load(Ordering::Relaxed) {
            *diagnostics = super::diagnostics(&observed, prepared.upstream.as_deref(), now_ms);
        }
        if !event.success {
            return;
        }
        let completed = observed
            .client
            .clone()
            .zip(prepared.upstream)
            .filter(|_| !observed.overlapping.load(Ordering::Relaxed))
            .map(|(client, upstream)| {
                Arc::new(CompletedRequest {
                    client,
                    upstream,
                    candidate,
                    completed_at_ms: now_ms,
                    sequence: observed.sequence,
                })
            });
        if let Some(scope) = observed.scope_key.and_then(|key| self.scopes.get_mut(&key)) {
            if let Some(completed) = completed {
                // Never let an older request overwrite a newer completed one.
                if scope
                    .previous
                    .as_ref()
                    .is_none_or(|previous| previous.sequence < completed.sequence)
                {
                    scope.previous = Some(completed);
                }
            }
            scope.last_touched_ms = now_ms;
        }
        self.remove_active(&request_key, observed.sequence);
    }
}

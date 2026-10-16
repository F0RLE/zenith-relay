//! Bounded, process-local comparisons for native Responses only.
//! No content, identity, salt, or digest is serialized or logged.

use super::{AccountTransport, ExecutorRoute, GatewayRuntime};
use crate::usage::{CacheContextBaseline, CacheContextDiagnostics, CacheContextScope};
use crate::{UsageEvent, WireApi};
use fingerprint::{identity, Fingerprint, RequestFingerprint};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

mod fingerprint;
mod store;
use store::{CompletedRequest, StoreState};

const BASELINE_TTL_MS: u64 = 30 * 60 * 1_000;
const MAX_SCOPES: usize = 256;
const MAX_ACTIVE_REQUESTS: usize = 128;
const MAX_SCOPE_BYTES: usize = 8_192;

pub(super) struct CacheContextStore {
    inner: Arc<StoreInner>,
}

struct StoreInner {
    salt: Option<[u8; 32]>,
    state: Mutex<StoreState>,
}

impl Default for CacheContextStore {
    fn default() -> Self {
        let mut salt = [0; 32];
        let salt = SystemRandom::new().fill(&mut salt).ok().map(|()| salt);
        Self {
            inner: Arc::new(StoreInner {
                salt,
                state: Mutex::default(),
            }),
        }
    }
}

/// Request-local ownership survives retries, SSE, and WS -> HTTP fallback.
/// Deliberately neither Debug nor Serialize.
#[derive(Clone)]
pub(crate) struct CacheContextObservation {
    inner: Arc<Observation>,
}

struct Observation {
    store: Weak<StoreInner>,
    request_key: Fingerprint,
    sequence: u64,
    scope_key: Option<Fingerprint>,
    scope: CacheContextScope,
    baseline: CacheContextBaseline,
    previous: Option<Arc<CompletedRequest>>,
    client: Option<Arc<RequestFingerprint>>,
    overlapping: AtomicBool,
}

impl Drop for Observation {
    fn drop(&mut self) {
        if let Some(store) = self.store.upgrade() {
            // An observation upgraded from Weak may be the last owner while
            // the store is locked. Do not re-enter that lock; prune removes
            // orphaned weak entries on the next request.
            let state = match store.state.try_lock() {
                Ok(state) => Some(state),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => None,
            };
            if let Some(mut state) = state {
                state.remove_active(&self.request_key, self.sequence);
            }
        }
    }
}

impl CacheContextStore {
    fn begin(
        &self,
        request: &Value,
        local_key_id: &str,
        request_id: &str,
        client_context: Option<&str>,
        now_ms: u64,
    ) -> CacheContextObservation {
        let salt = self.inner.salt.as_ref();
        let request_key = salt
            .map(|salt| {
                identity(
                    salt,
                    &[b"request", local_key_id.as_bytes(), request_id.as_bytes()],
                )
            })
            .unwrap_or_default();
        let scope = client_context
            .filter(|context| !context.is_empty())
            .map(|context| {
                (
                    CacheContextScope::ClientSession,
                    b"session".as_slice(),
                    context,
                )
            })
            .or_else(|| {
                request
                    .get("prompt_cache_key")
                    .and_then(Value::as_str)
                    .filter(|key| !key.trim().is_empty())
                    .map(|key| (CacheContextScope::CacheKey, b"cache".as_slice(), key))
            });
        let scope_key = salt
            .zip(scope)
            .filter(|(_, (_, _, value))| value.len() <= MAX_SCOPE_BYTES)
            .map(|(salt, (_, kind, value))| {
                identity(salt, &[kind, local_key_id.as_bytes(), value.as_bytes()])
            });
        let scope_kind = scope_key
            .and(scope.map(|(kind, _, _)| kind))
            .unwrap_or(CacheContextScope::Unavailable);
        let client = salt
            .filter(|_| scope_key.is_some())
            .and_then(|salt| RequestFingerprint::capture(request, salt))
            .map(Arc::new);
        let mut state = crate::poison::mutex(&self.inner.state);
        state.prune(now_ms);
        let (sequence, previous, tracked) = state.reserve(scope_key, request_key, now_ms);
        let observation = Arc::new(Observation {
            store: Arc::downgrade(&self.inner),
            request_key,
            sequence,
            scope_key: scope_key.filter(|_| tracked),
            scope: scope_kind,
            baseline: if salt.is_none() || !tracked {
                CacheContextBaseline::Unavailable
            } else if client.is_none() {
                CacheContextBaseline::SizeLimit
            } else if previous.is_some() {
                CacheContextBaseline::CompletedRequest
            } else {
                CacheContextBaseline::FirstObservation
            },
            previous,
            client,
            overlapping: AtomicBool::new(false),
        });
        if tracked {
            state.track(&observation, now_ms);
        }
        CacheContextObservation { inner: observation }
    }

    fn prepare(
        &self,
        observation: &CacheContextObservation,
        upstream: &Value,
        candidate_id: &str,
        now_ms: u64,
    ) -> CacheContextDiagnostics {
        let observed = &observation.inner;
        let upstream = self
            .inner
            .salt
            .as_ref()
            .filter(|_| observed.client.is_some())
            .and_then(|salt| RequestFingerprint::capture(upstream, salt))
            .map(Arc::new);
        let mut diagnostics = diagnostics(observed, upstream.as_deref(), now_ms);
        let candidate = self
            .inner
            .salt
            .as_ref()
            .map(|salt| identity(salt, &[b"candidate", candidate_id.as_bytes()]));
        let mut state = crate::poison::mutex(&self.inner.state);
        if let Some(active) = state
            .active
            .get_mut(&observed.request_key)
            .filter(|active| active.sequence == observed.sequence)
        {
            active.prepared = candidate.map(|candidate| store::PreparedRequest {
                candidate,
                upstream: upstream.clone(),
            });
            if diagnostics.baseline == CacheContextBaseline::CompletedRequest {
                diagnostics.candidate_changed = candidate
                    .zip(observed.previous.as_ref())
                    .map(|(candidate, previous)| candidate != previous.candidate);
            }
        }
        diagnostics
    }

    fn finish(&self, event: &mut UsageEvent, now_ms: u64) {
        let Some(salt) = self.inner.salt.as_ref() else {
            return;
        };
        let request_key = identity(
            salt,
            &[
                b"request",
                event.local_key_id.as_bytes(),
                event.request_id.as_bytes(),
            ],
        );
        let Some(candidate) = event
            .candidate_id
            .as_deref()
            .map(|candidate| identity(salt, &[b"candidate", candidate.as_bytes()]))
        else {
            return;
        };
        let mut state = crate::poison::mutex(&self.inner.state);
        state.finish(request_key, candidate, event, now_ms);
    }
}

fn diagnostics(
    observed: &Observation,
    upstream: Option<&RequestFingerprint>,
    now_ms: u64,
) -> CacheContextDiagnostics {
    let baseline = if observed.baseline == CacheContextBaseline::Unavailable {
        CacheContextBaseline::Unavailable
    } else if observed.client.is_none() || upstream.is_none() {
        CacheContextBaseline::SizeLimit
    } else if observed.overlapping.load(Ordering::Relaxed) {
        CacheContextBaseline::OverlappingRequests
    } else {
        observed.baseline
    };
    let previous = observed
        .previous
        .as_deref()
        .filter(|_| baseline == CacheContextBaseline::CompletedRequest);
    let client = observed.client.as_deref();
    CacheContextDiagnostics {
        baseline,
        scope: observed.scope,
        client_changes: client
            .zip(previous)
            .map(|(client, old)| client.changes_from(&old.client))
            .unwrap_or_default(),
        upstream_changes: upstream
            .zip(previous)
            .map(|(upstream, old)| upstream.changes_from(&old.upstream))
            .unwrap_or_default(),
        relay_changes: upstream
            .zip(client)
            .map(|(upstream, client)| upstream.changes_from(client))
            .unwrap_or_default(),
        client_history: client
            .map(|client| client.history(previous.map(|old| old.client.as_ref())))
            .unwrap_or_default(),
        upstream_history: upstream
            .map(|upstream| upstream.history(previous.map(|old| old.upstream.as_ref())))
            .unwrap_or_default(),
        relay_history: upstream
            .zip(client)
            .map(|(upstream, client)| upstream.relay_history(client))
            .unwrap_or_default(),
        candidate_changed: None,
        previous_completed_age_ms: previous.map(|old| now_ms.saturating_sub(old.completed_at_ms)),
    }
}

impl GatewayRuntime {
    pub(crate) fn begin_cache_context(
        &self,
        request: &Value,
        local_key_id: &str,
        request_id: &str,
        client_context: Option<&str>,
    ) -> CacheContextObservation {
        self.cache_context_store.begin(
            request,
            local_key_id,
            request_id,
            client_context,
            super::runtime_now_ms(),
        )
    }

    pub(crate) fn observe_cache_context(
        &self,
        observation: &CacheContextObservation,
        route: &mut ExecutorRoute,
        upstream: &Value,
    ) {
        if route.client_wire_api != WireApi::Responses
            || !route.adapter.is_passthrough()
            || route.account_transport != AccountTransport::NativeResponses
        {
            return;
        }
        if let Some(routing) = route.routing.as_mut() {
            routing.cache_context = Some(self.cache_context_store.prepare(
                observation,
                upstream,
                &route.candidate_id,
                super::runtime_now_ms(),
            ));
            route.cache_context_observation = Some(observation.clone());
        }
    }

    pub(crate) fn finish_cache_context(&self, event: &mut UsageEvent, now_ms: u64) {
        self.cache_context_store.finish(event, now_ms);
    }
}

#[cfg(test)]
mod tests;

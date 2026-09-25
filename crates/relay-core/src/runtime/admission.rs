//! Bounded wait ownership and oldest-compatible, principal-fair admission.
//! No request bodies or upstream credentials are stored in this queue.

use super::*;
use crate::scheduler::rotation::{
    AdmissionStopReason, RequestId, RotationOperation, SharedRequestBudget,
};
use crate::{Selection, SelectionRequest};
use std::collections::VecDeque;

#[cfg(test)]
mod tests;
mod waiting;

#[derive(Clone)]
pub(super) struct AdmissionRequest {
    pub key: AuthenticatedKey,
    pub model: String,
    pub protocols: Vec<WireApi>,
    pub tried: HashSet<String>,
    pub response_affinity: Option<String>,
    pub prompt_affinity: Option<String>,
    pub operation: RotationOperation,
    pub budget: SharedRequestBudget,
}

impl AdmissionRequest {
    pub fn selection<'a>(&'a self, scope: &'a CandidateScope, now_ms: u64) -> SelectionRequest<'a> {
        SelectionRequest {
            model: &self.model,
            allowed_protocols: &self.protocols,
            scope,
            tried: &self.tried,
            response_affinity_key: self.response_affinity.as_deref(),
            prompt_affinity_key: self.prompt_affinity.as_deref(),
            now_ms,
        }
    }

    fn retained_bytes(&self) -> usize {
        // Envelope estimates include parsing/repair copies. Add the bounded
        // queue metadata separately; never use serialized/compressed length.
        let metadata = self
            .tried
            .iter()
            .map(String::capacity)
            .fold(4096usize, usize::saturating_add)
            .saturating_add(self.model.capacity())
            .saturating_add(self.response_affinity.as_ref().map_or(0, String::capacity))
            .saturating_add(self.prompt_affinity.as_ref().map_or(0, String::capacity));
        self.budget.retained_input_bytes().saturating_add(metadata)
    }
}

pub(super) struct AdmissionLimits {
    pub requests: usize,
    pub principal_requests: usize,
    pub bytes: usize,
    pub principal_bytes: usize,
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            requests: 1024,
            principal_requests: 256,
            bytes: 256 * 1024 * 1024,
            principal_bytes: 128 * 1024 * 1024,
        }
    }
}

struct Waiter {
    request: AdmissionRequest,
    bytes: usize,
    selecting: bool,
}

#[derive(Default)]
pub(super) struct AdmissionQueue {
    pub limits: AdmissionLimits,
    waiters: VecDeque<Waiter>,
    principals: VecDeque<String>,
    retained_bytes: usize,
}

impl AdmissionQueue {
    fn insert(&mut self, request: AdmissionRequest, selecting: bool) -> bool {
        let id = request.budget.request_id();
        let bytes = request.retained_bytes();
        let principal = &request.key.id;
        let mut principal_count = 0usize;
        let mut principal_bytes = 0usize;
        for waiter in &self.waiters {
            if waiter.request.budget.request_id() == id {
                // Concurrent drivers must not count or admit the same request
                // twice. Sequential WS/HTTP handoff uses the same budget.
                return false;
            }
            if waiter.request.key.id == *principal {
                principal_count += 1;
                principal_bytes = principal_bytes.saturating_add(waiter.bytes);
            }
        }
        if self.waiters.len() >= self.limits.requests
            || principal_count >= self.limits.principal_requests
            || bytes > self.limits.bytes.saturating_sub(self.retained_bytes)
            || bytes > self.limits.principal_bytes.saturating_sub(principal_bytes)
            || !request.budget.begin_queue_wait()
        {
            return false;
        }
        if !self.principals.contains(principal) {
            self.principals.push_back(principal.clone());
        }
        self.retained_bytes = self.retained_bytes.saturating_add(bytes);
        self.waiters.push_back(Waiter {
            request,
            bytes,
            selecting,
        });
        true
    }

    fn turn(&self, runtime: &GatewayRuntime, now_ms: u64) -> Option<RequestId> {
        for principal in &self.principals {
            for waiter in self
                .waiters
                .iter()
                .filter(|waiter| waiter.selecting && waiter.request.key.id == *principal)
            {
                let request = &waiter.request;
                if request.budget.can_dispatch()
                    && request
                        .budget
                        .queue_deadline(runtime.route_recovery_enabled())
                        .is_none_or(|deadline| tokio::time::Instant::now() < deadline)
                    && runtime.admission_ready(request, now_ms)
                {
                    return Some(request.budget.request_id());
                }
            }
        }
        None
    }

    fn served(&mut self, principal: &str) {
        if let Some(index) = self.principals.iter().position(|id| id == principal) {
            let principal = self
                .principals
                .remove(index)
                .expect("principal index is current");
            self.principals.push_back(principal);
        }
    }

    fn remove(&mut self, id: RequestId) {
        let Some(index) = self
            .waiters
            .iter()
            .position(|waiter| waiter.request.budget.request_id() == id)
        else {
            return;
        };
        let waiter = self.waiters.remove(index).expect("waiter index is current");
        self.retained_bytes -= waiter.bytes;
        waiter.request.budget.finish_queue_wait();
        if !self
            .waiters
            .iter()
            .any(|other| other.request.key.id == waiter.request.key.id)
        {
            self.principals.retain(|id| *id != waiter.request.key.id);
        }
    }
}

/// Dropping a cancelled future removes its waiter synchronously, without a
/// spawned cleanup task, a health vote, or an upstream dispatch.
pub(super) struct AdmissionGuard<'a> {
    runtime: &'a GatewayRuntime,
    id: RequestId,
}

impl Drop for AdmissionGuard<'_> {
    fn drop(&mut self) {
        self.runtime
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(self.id);
        self.runtime.admission_changed.notify_waiters();
    }
}

impl GatewayRuntime {
    fn admission_ready(&self, request: &AdmissionRequest, now_ms: u64) -> bool {
        let scope = request.key.scope_read();
        self.lock_scheduler()
            .admission_ready_for(request.selection(&scope, now_ms), request.operation)
    }

    pub(super) fn register_waiter<'a>(
        &'a self,
        queue: &mut AdmissionQueue,
        request: &AdmissionRequest,
        selecting: bool,
    ) -> Option<AdmissionGuard<'a>> {
        if !queue.insert(request.clone(), selecting) {
            request
                .budget
                .stop_admission(AdmissionStopReason::QueueFull);
            return None;
        }
        Some(AdmissionGuard {
            runtime: self,
            id: request.budget.request_id(),
        })
    }

    pub(super) fn admission_deadline(
        &self,
        budget: &SharedRequestBudget,
    ) -> Result<Option<tokio::time::Instant>> {
        let deadline = budget.queue_deadline(self.route_recovery_enabled());
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            budget.stop_admission(AdmissionStopReason::WaitExpired);
            return Err(Error::Validation("request admission wait expired".into()));
        }
        Ok(deadline)
    }
}

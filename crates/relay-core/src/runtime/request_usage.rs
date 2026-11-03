//! Bounded active-request observations. No bodies, headers or credentials are retained.
use super::GatewayRuntime;
use crate::scheduler::rotation::{RequestWaitDiagnostics, SharedRequestBudget};
use crate::{AttemptOutcome, UsageEvent};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct RequestUsageRegistry(BTreeMap<String, TrackedRequest>);

struct TrackedRequest {
    started: Instant,
    budget: Option<SharedRequestBudget>,
    last: Option<UsageEvent>,
    pending: Option<PendingAttempt>,
}

struct PendingAttempt {
    event: UsageEvent,
    started: std::time::Instant,
}

pub(crate) struct RequestUsageScope {
    runtime: Arc<GatewayRuntime>,
    request_id: String,
    owned: bool,
}

impl GatewayRuntime {
    pub(crate) fn track_request_usage(
        self: &Arc<Self>,
        request_id: &str,
        budget: &SharedRequestBudget,
    ) -> RequestUsageScope {
        let mut registry = crate::poison::mutex(&self.control.request_usage);
        // Recursive compatibility repairs share the original clock and owner.
        let owned = registry.0.len() < 4096 && !registry.0.contains_key(request_id);
        if owned {
            registry.0.insert(
                request_id.to_owned(),
                TrackedRequest {
                    started: Instant::now(),
                    budget: self.diagnostics_enabled().then(|| budget.clone()),
                    last: None,
                    pending: None,
                },
            );
        }
        RequestUsageScope {
            runtime: self.clone(),
            request_id: request_id.to_owned(),
            owned,
        }
    }

    /// Retain one physical send until its response/stream owner emits usage.
    /// Cancellation before that handoff must not turn a real send into no attempt.
    pub(crate) fn track_dispatch_usage(&self, event: UsageEvent, started: std::time::Instant) {
        if let Some(request) = crate::poison::mutex(&self.control.request_usage)
            .0
            .get_mut(&event.request_id)
        {
            request.pending = Some(PendingAttempt { event, started });
        }
    }

    pub(crate) fn observe_request_usage(&self, event: &mut UsageEvent) {
        let mut registry = crate::poison::mutex(&self.control.request_usage);
        let Some(request) = registry.0.get_mut(&event.request_id) else {
            return;
        };
        if request
            .pending
            .as_ref()
            .is_some_and(|pending| pending.event.attempt == event.attempt)
        {
            request.pending = None;
        }
        let terminal = event.success
            || matches!(
                event
                    .routing
                    .as_ref()
                    .and_then(|routing| routing.attempt_outcome),
                Some(
                    AttemptOutcome::Completed
                        | AttemptOutcome::ReturnedFailure
                        | AttemptOutcome::ExecutionUncertain
                        | AttemptOutcome::ClientDisconnected
                )
            );
        apply_timing(
            event,
            request.started.elapsed().as_millis() as u64,
            false,
            request
                .budget
                .as_ref()
                .filter(|_| self.diagnostics_enabled())
                .and_then(SharedRequestBudget::wait_diagnostics),
        );
        if terminal {
            registry.0.remove(&event.request_id);
        } else {
            // Only one safe observation is retained, irrespective of retry count.
            request.last = Some(event.clone());
        }
    }

    fn finish_request_usage(&self, request_id: &str, status: u16, cancelled: bool) {
        let request = crate::poison::mutex(&self.control.request_usage)
            .0
            .remove(request_id);
        let Some(request) = request else { return };
        let (mut event, projection_only) = match request.pending {
            Some(pending) => {
                let mut event = pending.event;
                event.latency_ms = pending.started.elapsed().as_millis() as u64;
                (event, false)
            }
            None => {
                let Some(event) = request.last else { return };
                (event, true)
            }
        };
        if event
            .account_id
            .as_deref()
            .is_some_and(|id| !self.account_candidate_is_active(id))
        {
            return;
        }
        let Some(routing) = event.routing.as_mut() else {
            return;
        };
        routing.attempt_outcome = Some(if cancelled {
            AttemptOutcome::ClientDisconnected
        } else {
            AttemptOutcome::ReturnedFailure
        });
        event.success = false;
        event.http_status = status;
        if cancelled {
            event.error_category = Some(crate::error_codes::CLIENT_CANCELLED.to_owned());
            // The preceding provider rejection remains on its physical attempt,
            // not on the logical client-disconnection result.
            event.upstream_error = None;
        }
        apply_timing(
            &mut event,
            request.started.elapsed().as_millis() as u64,
            projection_only,
            request
                .budget
                .as_ref()
                .filter(|_| self.diagnostics_enabled())
                .and_then(SharedRequestBudget::wait_diagnostics),
        );
        if !projection_only {
            self.finish_cache_context(&mut event, super::runtime_now_ms());
        }
        // Cancellation is not a health/quota vote. An already recorded send is
        // updated only through its logical projection; a pending send is new.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.usage)(event)));
    }
}

fn apply_timing(
    event: &mut UsageEvent,
    duration_ms: u64,
    projection_only: bool,
    waiting: Option<RequestWaitDiagnostics>,
) {
    let Some(routing) = event.routing.as_mut() else {
        return;
    };
    let ttft_ms = if projection_only {
        routing
            .request_timing
            .as_ref()
            .and_then(|timing| timing.ttft_ms)
    } else {
        event.ttft_ms.map(|ttft| {
            duration_ms
                .saturating_sub(event.latency_ms)
                .saturating_add(ttft)
        })
    };
    routing.request_timing = Some(crate::scheduler::RequestTiming {
        duration_ms,
        ttft_ms,
        projection_only,
        waiting,
    });
}

impl RequestUsageScope {
    pub(crate) fn finish(mut self, status: u16, streaming: bool) {
        if self.owned && !streaming {
            self.runtime
                .finish_request_usage(&self.request_id, status, false);
        }
        // A streaming UsageStream owns completion/cancellation from this point.
        self.owned = false;
    }
}

impl Drop for RequestUsageScope {
    fn drop(&mut self) {
        if self.owned {
            self.runtime
                .finish_request_usage(&self.request_id, 502, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn usage_runtime() -> (Arc<GatewayRuntime>, Arc<Mutex<Vec<UsageEvent>>>) {
        usage_runtime_with_diagnostics(|| false)
    }

    fn usage_runtime_with_diagnostics(
        diagnostics_enabled: fn() -> bool,
    ) -> (Arc<GatewayRuntime>, Arc<Mutex<Vec<UsageEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let runtime = GatewayRuntime::from_pool(
            vec![crate::RuntimeSource::unrestricted(crate::ProviderSource {
                id: "synthetic".into(),
                name: "synthetic".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "synthetic-key".into(),
                proxy_url: None,
                wire_api: crate::WireApi::Responses,
                models: vec!["synthetic-model".into()],
            })],
            vec![crate::RuntimeLocalKey::unrestricted(
                crate::LocalGatewayKey {
                    id: "synthetic-local".into(),
                    secret: "synthetic-secret".into(),
                },
            )],
            crate::GatewayRuntimeOptions {
                diagnostics_enabled,
                ..Default::default()
            },
            Arc::new(move |event| sink.lock().unwrap().push(event)),
        )
        .unwrap();
        (Arc::new(runtime), events)
    }

    fn pending_event() -> UsageEvent {
        let mut event = crate::gateway::test_support::test_usage_event();
        event.success = false;
        event.input_tokens = None;
        event.output_tokens = None;
        event.routing = Some(
            serde_json::from_value(serde_json::json!({
                "reason":"only_eligible", "eligibleCandidates":1,
                "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0
            }))
            .unwrap(),
        );
        event
    }

    #[test]
    fn cancellation_before_dispatch_does_not_invent_a_send() {
        let (runtime, events) = usage_runtime();
        let scope =
            runtime.track_request_usage("not-sent", &SharedRequestBudget::for_incoming_request(3));
        drop(scope);
        assert!(events.lock().unwrap().is_empty());
        assert!(
            crate::poison::mutex(&runtime.control.request_usage)
                .0
                .is_empty()
        );
    }

    #[test]
    fn cancellation_settles_only_the_current_pending_send() {
        for second_send in [false, true] {
            let (runtime, events) = usage_runtime();
            let mut rejected = pending_event();
            rejected.attempt = 1;
            rejected.input_tokens = Some(5);
            rejected.output_tokens = Some(0);
            rejected.error_category = Some(crate::error_codes::UPSTREAM_RATE_LIMITED.into());
            rejected.routing.as_mut().unwrap().attempt_outcome =
                Some(AttemptOutcome::RetrySelection);
            let scope = runtime.track_request_usage(
                &rejected.request_id,
                &SharedRequestBudget::for_incoming_request(3),
            );
            runtime.track_dispatch_usage(rejected.clone(), std::time::Instant::now());
            runtime.observe_request_usage(&mut rejected);
            (runtime.usage)(rejected.clone());
            if second_send {
                let mut next = pending_event();
                next.attempt = 2;
                runtime.track_dispatch_usage(next, std::time::Instant::now());
            }
            drop(scope);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(!events[0].is_request_completion());
            assert_eq!(events[0].error_category, rejected.error_category);
            assert_eq!(events[0].input_tokens, Some(5));
            assert_eq!(events[1].is_request_completion(), !second_send);
            assert_eq!(events[1].attempt, if second_send { 2 } else { 1 });
            assert_eq!(events[1].input_tokens, (!second_send).then_some(5));
            assert_eq!(events[1].output_tokens, (!second_send).then_some(0));
            assert_eq!(
                events[1].error_category.as_deref(),
                Some("client_cancelled")
            );
            assert_eq!(
                events[1].routing.as_ref().unwrap().attempt_outcome,
                Some(AttemptOutcome::ClientDisconnected)
            );
            assert!(
                crate::poison::mutex(&runtime.control.request_usage)
                    .0
                    .is_empty()
            );
        }
    }

    #[test]
    fn settled_send_has_no_second_cancellation_owner() {
        let (runtime, events) = usage_runtime();
        let mut event = pending_event();
        let scope = runtime.track_request_usage(
            &event.request_id,
            &SharedRequestBudget::for_incoming_request(3),
        );
        runtime.track_dispatch_usage(event.clone(), std::time::Instant::now());
        event.routing.as_mut().unwrap().attempt_outcome = Some(AttemptOutcome::ReturnedFailure);
        runtime.observe_request_usage(&mut event);
        (runtime.usage)(event);
        drop(scope);
        assert_eq!(events.lock().unwrap().len(), 1);
        assert!(
            crate::poison::mutex(&runtime.control.request_usage)
                .0
                .is_empty()
        );
    }

    #[test]
    fn logical_timing_includes_waits_without_changing_physical_measurements() {
        let mut event = crate::gateway::test_support::test_usage_event();
        event.routing = Some(
            serde_json::from_value(serde_json::json!({
                "reason":"only_eligible", "eligibleCandidates":1,
                "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0
            }))
            .unwrap(),
        );
        event.latency_ms = 6_000;
        event.ttft_ms = Some(1_000);
        event.generation_ms = Some(5_000);
        apply_timing(&mut event, 14_000, false, None);
        let timing = event
            .routing
            .as_ref()
            .unwrap()
            .request_timing
            .as_ref()
            .unwrap();
        assert_eq!((timing.duration_ms, timing.ttft_ms), (14_000, Some(9_000)));
        assert_eq!((event.latency_ms, event.ttft_ms), (6_000, Some(1_000)));
        apply_timing(&mut event, 30_000, true, None);
        let timing = event
            .routing
            .as_ref()
            .unwrap()
            .request_timing
            .as_ref()
            .unwrap();
        assert_eq!((timing.duration_ms, timing.ttft_ms), (30_000, Some(9_000)));
        assert_eq!(event.generation_ms, Some(5_000));
        assert!(event.is_request_completion());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_recovery_keeps_live_waits_without_inventing_an_attempt() {
        use crate::scheduler::rotation::RequestWaitKind;
        use std::time::Duration;

        let (runtime, events) = usage_runtime_with_diagnostics(|| true);
        let budget = SharedRequestBudget::for_incoming_request(3);
        let mut rejected = pending_event();
        rejected.routing.as_mut().unwrap().attempt_outcome = Some(AttemptOutcome::RetrySelection);
        let scope = runtime.track_request_usage(&rejected.request_id, &budget);
        runtime.observe_request_usage(&mut rejected);
        (runtime.usage)(rejected.clone());
        assert!(budget.begin_queue_wait(RequestWaitKind::PoolEvent));
        tokio::time::advance(Duration::from_millis(2_500)).await;
        // A nested repair scope cannot reset the original clock or own finalization.
        drop(runtime.track_request_usage(&rejected.request_id, &budget.clone()));
        assert_eq!(events.lock().unwrap().len(), 1);
        drop(scope);
        budget.finish_queue_wait();
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[1].is_request_completion());
        assert_eq!(events[1].attempt, rejected.attempt);
        let timing = events[1]
            .routing
            .as_ref()
            .unwrap()
            .request_timing
            .as_ref()
            .unwrap();
        let waiting = timing.waiting.as_ref().unwrap();
        assert_eq!(timing.duration_ms, 2_500);
        assert_eq!(waiting.total_ms, 2_500);
        assert_eq!(waiting.pool_event.as_ref().unwrap().count, 1);
        assert_eq!(waiting.pool_event.as_ref().unwrap().duration_ms, 2_500);
        assert!(waiting.recovery.is_none());
        assert_eq!(budget.dispatches(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn successful_wait_snapshot_respects_diagnostic_opt_out_and_physical_timing() {
        use crate::scheduler::rotation::RequestWaitKind;
        use std::time::Duration;

        for enabled in [false, true] {
            let (runtime, events) =
                usage_runtime_with_diagnostics(if enabled { || true } else { || false });
            let budget = SharedRequestBudget::for_incoming_request(3);
            let mut event = pending_event();
            let scope = runtime.track_request_usage(&event.request_id, &budget);
            assert!(budget.begin_queue_wait(RequestWaitKind::Capacity));
            tokio::time::advance(Duration::from_millis(80)).await;
            budget.finish_queue_wait();
            tokio::time::advance(Duration::from_millis(42)).await;
            event.success = true;
            event.latency_ms = 42;
            event.ttft_ms = Some(10);
            runtime.observe_request_usage(&mut event);
            drop(scope);
            let timing = event
                .routing
                .as_ref()
                .unwrap()
                .request_timing
                .as_ref()
                .unwrap();
            assert_eq!(timing.duration_ms, 122);
            assert_eq!(timing.ttft_ms, Some(90));
            assert_eq!((event.latency_ms, event.ttft_ms), (42, Some(10)));
            assert_eq!(timing.waiting.is_some(), enabled);
            if let Some(waiting) = &timing.waiting {
                assert_eq!(waiting.total_ms, 80);
                assert_eq!(waiting.capacity.as_ref().unwrap().count, 1);
            }
            assert!(events.lock().unwrap().is_empty());
        }
    }
}

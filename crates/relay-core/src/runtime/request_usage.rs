//! Bounded active-request observations. No bodies, headers or credentials are retained.
use super::GatewayRuntime;
use crate::scheduler::RequestTimingPhases;
use crate::scheduler::rotation::{RequestWaitDiagnostics, SharedRequestBudget};
use crate::{AttemptOutcome, UsageEvent};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct RequestUsageRegistry(BTreeMap<String, TrackedRequest>);

struct TrackedRequest {
    started: Instant,
    first_executor: Option<ExecutorTiming>,
    latest_executor_started: Option<Instant>,
    ingress: RequestIngressTiming,
    selection_ms: Option<u64>,
    budget: Option<SharedRequestBudget>,
    last: Option<UsageEvent>,
    pending: Option<PendingAttempt>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct RequestIngressTiming {
    pub(crate) body_read_and_parse_ms: Option<u64>,
    pub(crate) bps_access_refresh_ms: Option<u64>,
    pub(crate) cache_context_ms: Option<u64>,
}

struct ExecutorTiming {
    started: Instant,
    preparation_ms: Option<u64>,
}

impl TrackedRequest {
    fn phases(&self) -> Option<RequestTimingPhases> {
        let first = self.first_executor.as_ref()?;
        let offset = |start: Instant| {
            start
                .checked_duration_since(self.started)
                .map(|duration| duration.as_millis() as u64)
        };
        let pre_attempt_ms = offset(first.started)?;
        let latest_offset_ms = offset(self.latest_executor_started?)?;
        Some(RequestTimingPhases {
            pre_attempt_ms,
            prior_attempts_ms: Some(latest_offset_ms.checked_sub(pre_attempt_ms)?),
            body_read_and_parse_ms: self.ingress.body_read_and_parse_ms,
            bps_access_refresh_ms: self.ingress.bps_access_refresh_ms,
            cache_context_ms: self.ingress.cache_context_ms,
            candidate_selection_ms: self.selection_ms,
            request_preparation_ms: first.preparation_ms,
            ..Default::default()
        })
    }
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
        self.track_request_usage_from(request_id, budget, Instant::now(), Default::default())
    }

    pub(crate) fn track_request_usage_from(
        self: &Arc<Self>,
        request_id: &str,
        budget: &SharedRequestBudget,
        started: Instant,
        ingress: RequestIngressTiming,
    ) -> RequestUsageScope {
        let mut registry = crate::poison::mutex(&self.control.request_usage);
        // Recursive compatibility repairs share the original clock and owner.
        let owned = registry.0.len() < 4096 && !registry.0.contains_key(request_id);
        if owned {
            registry.0.insert(
                request_id.to_owned(),
                TrackedRequest {
                    started,
                    first_executor: None,
                    latest_executor_started: None,
                    ingress,
                    selection_ms: None,
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

    pub(crate) fn record_request_selection(&self, request_id: &str, started: std::time::Instant) {
        let elapsed = started.elapsed().as_millis() as u64;
        if let Some(request) = crate::poison::mutex(&self.control.request_usage)
            .0
            .get_mut(request_id)
            && request.first_executor.is_none()
        {
            request.selection_ms = Some(elapsed);
        }
    }

    /// Retain one physical send until its response/stream owner emits usage.
    /// Cancellation before that handoff must not turn a real send into no attempt.
    pub(crate) fn track_dispatch_usage(&self, event: UsageEvent, started: std::time::Instant) {
        if let Some(request) = crate::poison::mutex(&self.control.request_usage)
            .0
            .get_mut(&event.request_id)
        {
            let executor_started = Instant::from_std(started);
            request
                .first_executor
                .get_or_insert_with(|| ExecutorTiming {
                    started: executor_started,
                    preparation_ms: event
                        .routing
                        .as_ref()
                        .and_then(|routing| routing.transport.as_ref()?.request_preparation_ms),
                });
            request.latest_executor_started = Some(executor_started);
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
            request.phases(),
        );
        if terminal {
            registry.0.remove(&event.request_id);
        } else {
            // Only one safe observation is retained, irrespective of retry count.
            request.last = Some(event.clone());
        }
    }

    /// Amend the current physical send without emitting usage or resetting its clock.
    pub(crate) fn observe_pending_dispatch(
        &self,
        request_id: &str,
        attempt: u16,
        observe: impl FnOnce(&mut UsageEvent),
    ) {
        let mut registry = crate::poison::mutex(&self.control.request_usage);
        let Some(pending) = registry
            .0
            .get_mut(request_id)
            .and_then(|request| request.pending.as_mut())
            .filter(|pending| pending.event.attempt == attempt)
        else {
            return;
        };
        observe(&mut pending.event);
    }

    fn finish_request_usage(&self, request_id: &str, status: u16, cancelled: bool) {
        let request = crate::poison::mutex(&self.control.request_usage)
            .0
            .remove(request_id);
        let Some(request) = request else { return };
        let phases = request.phases();
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
        event.normalize_client_cancellation();
        apply_timing(
            &mut event,
            request.started.elapsed().as_millis() as u64,
            projection_only,
            request
                .budget
                .as_ref()
                .filter(|_| self.diagnostics_enabled())
                .and_then(SharedRequestBudget::wait_diagnostics),
            phases,
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
    mut phases: Option<RequestTimingPhases>,
) {
    let Some(routing) = event.routing.as_mut() else {
        return;
    };
    if projection_only {
        phases = routing
            .request_timing
            .as_mut()
            .and_then(|timing| timing.phases.take());
    }
    if let Some(phases) = phases.as_mut() {
        let executor_offset = phases.executor_offset_ms();
        let absolute = |offset: u64| {
            executor_offset
                .and_then(|start| start.checked_add(offset))
                .filter(|time| *time <= duration_ms)
        };
        phases.post_attempt_ms = absolute(event.latency_ms).map(|end| duration_ms - end);
        if let Some(transport) = routing.transport.as_ref() {
            if !projection_only {
                phases.upstream_ttft_ms = transport
                    .stream_response
                    .as_ref()
                    .and_then(|stream| stream.upstream_output_first_ms)
                    .and_then(absolute);
                phases.client_first_output_ms = transport.client_first_output_ms.and_then(absolute);
            }
            phases.after_client_ready_ms = transport
                .client_response_ready_ms
                .and_then(absolute)
                .map(|ready| duration_ms - ready);
        }
    }
    let ttft_ms = if projection_only {
        routing
            .request_timing
            .as_ref()
            .and_then(|timing| timing.ttft_ms)
    } else if let Some(phases) = &phases {
        event
            .ttft_ms
            .and_then(|ttft| phases.executor_offset_ms()?.checked_add(ttft))
            .filter(|ttft| *ttft <= duration_ms)
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
        phases: phases.filter(|_| routing.transport.is_some()),
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
            rejected.http_status = 429;
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
            assert_eq!(events[0].http_status, 429);
            assert_eq!(events[0].error_origin(), Some(crate::ErrorOrigin::Provider));
            assert_eq!(events[0].input_tokens, Some(5));
            assert_eq!(events[1].is_request_completion(), !second_send);
            assert_eq!(events[1].attempt, if second_send { 2 } else { 1 });
            assert_eq!(events[1].input_tokens, (!second_send).then_some(5));
            assert_eq!(events[1].output_tokens, (!second_send).then_some(0));
            assert_eq!(events[1].http_status, 499);
            assert_eq!(events[1].error_origin(), Some(crate::ErrorOrigin::Relay));
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
        apply_timing(&mut event, 14_000, false, None, None);
        let timing = event
            .routing
            .as_ref()
            .unwrap()
            .request_timing
            .as_ref()
            .unwrap();
        assert_eq!((timing.duration_ms, timing.ttft_ms), (14_000, Some(9_000)));
        assert_eq!((event.latency_ms, event.ttft_ms), (6_000, Some(1_000)));
        apply_timing(&mut event, 30_000, true, None, None);
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

    #[test]
    fn request_clock_distinguishes_observed_upstream_ttft_from_unclassified_ttft() {
        for observed in [false, true] {
            let mut event = pending_event();
            event.ttft_ms = Some(2_500);
            event.latency_ms = 6_800;
            let routing = event.routing.as_mut().unwrap();
            routing.transport = Some(
                serde_json::from_value(serde_json::json!({
                    "upstream":"websocket", "mode":"native",
                    "upstreamResponseMs":6_800, "clientFirstOutputMs":6_810,
                    "clientResponseReadyMs":6_830,
                    "streamResponse":{"upstreamOutputFirstMs": observed.then_some(2_000)}
                }))
                .unwrap(),
            );
            apply_timing(
                &mut event,
                6_900,
                false,
                None,
                Some(RequestTimingPhases {
                    pre_attempt_ms: 60,
                    ..Default::default()
                }),
            );
            let timing = event
                .routing
                .as_ref()
                .unwrap()
                .request_timing
                .as_ref()
                .unwrap();
            let phases = timing.phases.as_ref().unwrap();
            assert_eq!(timing.ttft_ms, Some(2_560));
            assert_eq!(phases.upstream_ttft_ms, observed.then_some(2_060));
            assert_eq!(phases.client_first_output_ms, Some(6_870));
            assert_eq!(phases.post_attempt_ms, Some(40));
            assert_eq!(phases.after_client_ready_ms, Some(10));
            assert_eq!(
                phases.pre_attempt_ms + event.latency_ms + phases.post_attempt_ms.unwrap(),
                6_900
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retry_timing_uses_first_preparation_and_latest_executor_clock() {
        use serde_json::json;
        use std::time::Duration;

        let (runtime, _) = usage_runtime();
        let mut event = pending_event();
        let request_started = Instant::now();
        let scope = runtime.track_request_usage_from(
            &event.request_id,
            &SharedRequestBudget::for_incoming_request(3),
            request_started,
            RequestIngressTiming {
                bps_access_refresh_ms: Some(790),
                ..Default::default()
            },
        );
        runtime.record_request_selection(&event.request_id, std::time::Instant::now());
        let mut first_selection = None;
        let mut previous_end = 0;
        for (attempt, offset, latency, preparation) in [
            (1, 825, 5_195, 20),
            (2, 6_075, 5_293, 16),
            (3, 11_424, 59_060, 14),
        ] {
            event.attempt = attempt;
            event.latency_ms = latency;
            event.success = attempt == 3;
            event.http_status = if event.success { 200 } else { 429 };
            event.ttft_ms = event.success.then_some(7_847);
            let routing = event.routing.as_mut().unwrap();
            routing.attempt_outcome = Some(if event.success {
                AttemptOutcome::Completed
            } else {
                AttemptOutcome::RetrySelection
            });
            routing.transport = Some(
                serde_json::from_value(json!({
                    "upstream":"websocket","mode":"native","requestPreparationMs":preparation,
                    "clientFirstOutputMs":event.success.then_some(59_104),
                    "clientResponseReadyMs":event.success.then_some(59_127),
                    "streamResponse":{"upstreamOutputFirstMs":event.ttft_ms}
                }))
                .unwrap(),
            );
            let executor_started = request_started + Duration::from_millis(offset);
            if attempt > 1 {
                runtime.record_request_selection(
                    &event.request_id,
                    std::time::Instant::now() - Duration::from_secs(1),
                );
            }
            runtime.track_dispatch_usage(event.clone(), executor_started.into_std());
            let end = offset + latency + if event.success { 68 } else { 0 };
            tokio::time::advance(Duration::from_millis(end - previous_end)).await;
            previous_end = end;
            runtime.observe_request_usage(&mut event);
            let timing = event
                .routing
                .as_ref()
                .unwrap()
                .request_timing
                .as_ref()
                .unwrap();
            let phases = timing.phases.as_ref().unwrap();
            assert_eq!(phases.pre_attempt_ms, 825);
            assert_eq!(phases.prior_attempts_ms, Some(offset - 825));
            assert_eq!(phases.request_preparation_ms, Some(20));
            if attempt == 1 {
                first_selection = phases.candidate_selection_ms;
                assert!(first_selection.is_some());
            }
            assert_eq!(phases.candidate_selection_ms, first_selection);
            assert_eq!(phases.bps_access_refresh_ms, Some(790));
            assert_eq!(
                phases.pre_attempt_ms
                    + phases.prior_attempts_ms.unwrap()
                    + latency
                    + phases.post_attempt_ms.unwrap(),
                timing.duration_ms
            );
            if event.success {
                assert_eq!(timing.duration_ms, 70_552);
                assert_eq!(timing.ttft_ms, Some(19_271));
                assert_eq!(phases.upstream_ttft_ms, Some(19_271));
                assert_eq!(phases.client_first_output_ms, Some(70_528));
                assert_eq!(phases.after_client_ready_ms, Some(1));
            }
        }
        drop(scope);
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

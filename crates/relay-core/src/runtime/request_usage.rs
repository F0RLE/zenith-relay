//! Bounded active-request observations. No bodies, headers or credentials are retained.
use super::GatewayRuntime;
use crate::{AttemptOutcome, UsageEvent};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct RequestUsageRegistry(BTreeMap<String, TrackedRequest>);

struct TrackedRequest {
    started: Instant,
    last: Option<UsageEvent>,
}

pub(crate) struct RequestUsageScope {
    runtime: Arc<GatewayRuntime>,
    request_id: String,
    owned: bool,
}

impl GatewayRuntime {
    pub(crate) fn track_request_usage(self: &Arc<Self>, request_id: &str) -> RequestUsageScope {
        let mut registry = crate::poison::mutex(&self.control.request_usage);
        // Recursive compatibility repairs share the original clock and owner.
        let owned = registry.0.len() < 4096 && !registry.0.contains_key(request_id);
        if owned {
            registry.0.insert(
                request_id.to_owned(),
                TrackedRequest {
                    started: Instant::now(),
                    last: None,
                },
            );
        }
        RequestUsageScope {
            runtime: self.clone(),
            request_id: request_id.to_owned(),
            owned,
        }
    }

    pub(crate) fn observe_request_usage(&self, event: &mut UsageEvent) {
        let mut registry = crate::poison::mutex(&self.control.request_usage);
        let Some(request) = registry.0.get_mut(&event.request_id) else {
            return;
        };
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
        apply_timing(event, request.started.elapsed().as_millis() as u64, false);
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
        let Some(mut event) = request.last else {
            return;
        };
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
            true,
        );
        // Do not reapply runtime quota, health, cache or account observations.
        // The sink updates only the logical projection of the existing attempt.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.usage)(event)));
    }
}

fn apply_timing(event: &mut UsageEvent, duration_ms: u64, projection_only: bool) {
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
        apply_timing(&mut event, 14_000, false);
        let timing = event
            .routing
            .as_ref()
            .unwrap()
            .request_timing
            .as_ref()
            .unwrap();
        assert_eq!((timing.duration_ms, timing.ttft_ms), (14_000, Some(9_000)));
        assert_eq!((event.latency_ms, event.ttft_ms), (6_000, Some(1_000)));
        apply_timing(&mut event, 30_000, true);
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
}

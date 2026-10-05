use super::prelude::*;
mod repair;
mod settle;

pub(super) enum FailureStep {
    Continue,
    Break,
    Respond(Response<Body>),
}

pub(super) struct UpstreamFailureInput<'a> {
    pub(super) upstream: reqwest::Response,
    pub(super) status: StatusCode,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) lease: &'a CandidateLease,
    pub(super) route: &'a crate::runtime::ExecutorRoute,
    pub(super) source_model: &'a str,
    pub(super) request_id: &'a str,
    pub(super) attempt: u16,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) reasoning_effort: &'a ReasoningEffortDiagnostics,
    pub(super) requested_model: &'a str,
    pub(super) tool_use: &'a crate::usage::ToolUseDiagnostics,
    pub(super) started: Instant,
    pub(super) carry: RejectionCarry<'a>,
}

/// Fields that survive from the failed attempt into repair and settlement.
pub(super) struct RejectionCarry<'a> {
    pub(super) client_wire_api: WireApi,
    pub(super) request: &'a mut Value,
    pub(super) adapter_is_passthrough: bool,
    pub(super) has_previous_response_id: bool,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) has_unpaired_tool_output: &'a mut bool,
    pub(super) requires_affinity_owner: &'a mut bool,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_failure_origin: &'a mut ErrorOrigin,
    pub(super) last_preserved_upstream_error: &'a mut Option<PreservedUpstreamError>,
    pub(super) tool_policy: &'a mut RequestToolPolicy,
    pub(super) stream: bool,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) resolved_model: &'a str,
    pub(super) allow_previous_response_reset: bool,
    pub(super) response_affinity_hit: bool,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) prompt_affinity_key: &'a Option<String>,
    pub(super) confirmed_response_missing: &'a mut bool,
    pub(super) account_route: bool,
    pub(super) forwarded_headers: &'a HeaderMap,
}

/// One collected upstream rejection, shared by repair and final settlement.
///
/// The attempt loop fills the fields by name. Repair may change the request and
/// then return the same value; settlement consumes the body.
pub(super) struct CollectedRejection<'a> {
    pub(super) status: StatusCode,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) lease: &'a CandidateLease,
    pub(super) route: &'a crate::runtime::ExecutorRoute,
    pub(super) source_model: &'a str,
    pub(super) request_id: &'a str,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) carry: RejectionCarry<'a>,
    pub(super) event: &'a mut UsageEvent,
    pub(super) bytes: Vec<u8>,
}

/// Classify one unsuccessful upstream response. Repairs and recoverable route
/// failures continue the attempt loop; a terminal response leaves it.
pub(super) async fn handle_upstream_failure(input: UpstreamFailureInput<'_>) -> FailureStep {
    let UpstreamFailureInput {
        upstream,
        status,
        response_headers,
        runtime,
        lease,
        route,
        source_model,
        request_id,
        attempt,
        key,
        reasoning_effort,
        requested_model,
        tool_use,
        started,
        carry:
            RejectionCarry {
                client_wire_api,
                request,
                adapter_is_passthrough,
                has_previous_response_id,
                repairs,
                tried,
                has_unpaired_tool_output,
                requires_affinity_owner,
                last_failure,
                last_failure_origin,
                last_preserved_upstream_error,
                tool_policy,
                stream,
                response_affinity_key,
                resolved_model,
                allow_previous_response_reset,
                response_affinity_hit,
                selected_error_origin,
                prompt_affinity_key,
                confirmed_response_missing,
                account_route,
                forwarded_headers,
            },
    } = input;
    let mut event = usage_event(
        UsageAttempt {
            request_id,
            attempt,
            local_key_id: &key.id,
            route,
            reasoning_effort: Some(reasoning_effort),
            requested_model,
            tool_use: tool_use.clone(),
        },
        false,
        status.as_u16(),
        None,
        started.elapsed().as_millis() as u64,
    );
    let bytes = match crate::transport::collect(upstream).await {
        Ok(bytes) => bytes,
        Err(_) if retryable_status(status, has_previous_response_id) => {
            let failure = AttemptFailure::status_with_body(status, None);
            event.error_category = Some(failure.category.to_string());
            let state = settle_status_failure(
                runtime,
                lease,
                source_model,
                status,
                failure.category,
                response_headers,
                None,
            );
            apply_failure_state(&mut event, state);
            emit_usage(runtime, event);
            *last_failure = Some(failure);
            *last_failure_origin = selected_error_origin;
            return FailureStep::Continue;
        }
        Err(_) => {
            lease.settle_rotation_unknown(now_ms());
            return FailureStep::Respond(upstream_body_error_response(runtime, event, started));
        }
    };

    let rejection = CollectedRejection {
        status,
        response_headers,
        runtime,
        lease,
        route,
        source_model,
        request_id,
        key,
        carry: RejectionCarry {
            client_wire_api,
            request,
            adapter_is_passthrough,
            has_previous_response_id,
            repairs,
            tried,
            has_unpaired_tool_output,
            requires_affinity_owner,
            last_failure,
            last_failure_origin,
            last_preserved_upstream_error,
            tool_policy,
            stream,
            response_affinity_key,
            resolved_model,
            allow_previous_response_reset,
            response_affinity_hit,
            selected_error_origin,
            prompt_affinity_key,
            confirmed_response_missing,
            account_route,
            forwarded_headers,
        },
        event: &mut event,
        bytes,
    };
    match repair::repair_collected_rejection(rejection) {
        repair::AfterRepair::Step(step) => step,
        repair::AfterRepair::Proceed(rejection, failure) => {
            settle::settle_collected_rejection(rejection, failure)
        }
    }
}

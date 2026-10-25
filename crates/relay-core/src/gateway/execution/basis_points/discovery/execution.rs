use super::super::structured_output::requires_buffering;
use super::{capture, DiscoverySession};
use crate::gateway::errors::{apply_failure_state, settle_route_failure, AttemptFailure};
use crate::gateway::execution::{
    attempt_error_response, request::adapter_error_response_for_origin,
};
use crate::gateway::response::{
    completed_upstream_response, emit_usage, populate_tokens, usage_event, UsageAttempt,
};
use crate::runtime::{
    AuthenticatedKey, AuthorizationDispatch, AuthorizationIdentityPolicy, AuthorizedResponse,
    CandidateLease, ExecutorRoute,
};
use crate::scheduler::rotation::SharedRequestBudget;
use crate::usage::{ReasoningEffortDiagnostics, ToolUseDiagnostics, UsageEvent};
use crate::{ErrorOrigin, GatewayRuntime};
use axum::body::{Body, Bytes};
use axum::http::{header, Response, StatusCode};
use serde_json::Value;
use std::time::Instant;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(in crate::gateway) struct StreamingDiscovery {
    pub session: DiscoverySession,
    pub upstream: Value,
    pub owner: crate::runtime::AuthorizationIncarnation,
    pub budget: SharedRequestBudget,
}

pub(in crate::gateway::execution) struct DiscoveryResponse {
    pub response: reqwest::Response,
    pub streaming: Option<StreamingDiscovery>,
}

impl From<reqwest::Response> for DiscoveryResponse {
    fn from(response: reqwest::Response) -> Self {
        Self {
            response,
            streaming: None,
        }
    }
}

pub(in crate::gateway::execution) fn prepare_dispatch(
    original: &Value,
    upstream: &Value,
    budget: &SharedRequestBudget,
    tool_use: &mut ToolUseDiagnostics,
    runtime: &GatewayRuntime,
    local_key_id: &str,
    client_context_id: Option<&str>,
) -> Result<(Value, Option<DiscoverySession>), crate::AdapterError> {
    capture(original, tool_use);
    let mut session = DiscoverySession::new(original, tool_use.client_context.is_some())?;
    if session.is_some() && budget.with_budget(|budget| budget.remaining()) < 2 {
        let mut fallback = original.clone();
        fallback[super::super::presentation::OPTION] = serde_json::json!("compact");
        fallback["model"] = upstream["model"].clone();
        capture(&fallback, tool_use);
        return super::super::prepare_request(&fallback).map(|body| (body, None));
    }
    let mut upstream = upstream.clone();
    let model = original["model"].as_str().unwrap_or_default();
    let prefix_scope = runtime
        .basis_points_catalog_store
        .prefix_scope(original, client_context_id);
    if let Some(session) = &mut session {
        session.preload_checkpoint = runtime.basis_points_catalog_store.checkpoint_for_history(
            local_key_id,
            model,
            &upstream["input"],
        );
        session.prompt_prefix = prefix_scope.and_then(|scope| {
            super::super::prepare::prepared_tool_instructions(original, &upstream)
                .map(|text| (scope, text.to_owned()))
        });
        let loaded = runtime.basis_points_catalog_store.load(
            local_key_id,
            model,
            &upstream["input"],
            crate::gateway::now_ms(),
        );
        session.preload(&loaded, &mut upstream)?;
        session.record(tool_use);
    } else if original["tools"].as_array().is_none_or(Vec::is_empty) {
        if let Some(prefix) = prefix_scope.and_then(|scope| {
            runtime.basis_points_catalog_store.load_prefix(
                local_key_id,
                model,
                &upstream["input"],
                &scope,
                crate::gateway::now_ms(),
            )
        }) {
            super::super::prepare::restore_text_only_prefix(original, &mut upstream, prefix);
        }
    }
    Ok((upstream, session))
}

pub(in crate::gateway::execution) struct DiscoveryExecution<'a> {
    pub session: Option<DiscoverySession>,
    pub allow_streaming: bool,
    pub runtime: &'a GatewayRuntime,
    pub key: &'a AuthenticatedKey,
    pub lease: &'a CandidateLease,
    pub budget: &'a SharedRequestBudget,
    pub route: &'a mut ExecutorRoute,
    pub request_id: &'a str,
    pub requested_model: &'a str,
    pub reasoning_effort: &'a ReasoningEffortDiagnostics,
    pub tool_use: &'a mut ToolUseDiagnostics,
    pub selected_error_origin: ErrorOrigin,
    pub started: Instant,
}

/// No task is spawned: cancellation drops the read/send future and its body.
/// Once a local lookup has completed, every failure returns directly instead
/// of entering the outer candidate retry/rotation loop.
#[allow(clippy::result_large_err)]
pub(in crate::gateway::execution) async fn resolve(
    first: AuthorizedResponse,
    original: &Value,
    mut upstream: Value,
    mut context: DiscoveryExecution<'_>,
) -> Result<DiscoveryResponse, Response<Body>> {
    context.route.basis_points_discovery = context.session.is_some();
    if !first.response.status().is_success() {
        return Ok(first.response.into());
    }
    let Some(mut session) = context.session.take() else {
        return Ok(first.response.into());
    };
    let streamed = first
        .response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|mime| {
            mime.split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    if context.allow_streaming
        && streamed
        && original.get("stream").and_then(Value::as_bool) == Some(true)
        && (session.is_native() || !requires_buffering(original))
    {
        return Ok(DiscoveryResponse {
            response: first.response,
            streaming: Some(StreamingDiscovery {
                session,
                upstream,
                owner: first.authorization_incarnation,
                budget: context.budget.clone(),
            }),
        });
    }
    let expected_owner = first.authorization_incarnation;
    let mut response = first.response;
    loop {
        context.route.observe_http_payload(&response);
        let status = response.status();
        let headers = response.headers().clone();
        context
            .route
            .observe_http_response(&headers, context.started.elapsed().as_millis() as u64);
        context.tool_use.upstream_tool_call_count = None;
        let mut event = context.event(status);
        let expected_model = context
            .runtime
            .block_degraded_routes_enabled()
            .then_some(context.route.source_model.as_str());
        let raw = match crate::transport::collect_with_progress_limit(
            response,
            if session.is_native() {
                None
            } else {
                crate::transport::basis_points_progress_timeout()
            },
            MAX_RESPONSE_BYTES,
            |frame| crate::gateway::response::stop_basis_points_event(frame, expected_model),
        )
        .await
        {
            Ok(raw) => raw,
            Err(partial) => {
                populate_tokens(&mut event, &partial.bytes);
                let failure = AttemptFailure::stream(if partial.timed_out {
                    crate::error_codes::STREAM_IDLE_TIMEOUT
                } else {
                    crate::error_codes::STREAM_ERROR
                });
                return Err(context.fail(event, failure, &headers));
            }
        };
        populate_tokens(&mut event, &raw);
        if !status.is_success() {
            // A continuation's rejected response is terminal for this logical
            // request. Do not replay earlier accepted generations elsewhere.
            let failure =
                AttemptFailure::status_with_body(status, Some(&raw)).with_retry_headers(&headers);
            event.upstream_error = Some(crate::usage::UpstreamErrorDetails::from_response(
                Some(status.as_u16()),
                &raw,
                &headers,
            ));
            return Err(context.fail(event, failure, &headers));
        }
        let bytes = match completed_upstream_response(&raw, true, expected_model) {
            Ok(bytes) => bytes,
            Err(failure) => {
                event.upstream_error = failure.upstream_error.map(|mut details| {
                    details.http_status = Some(status.as_u16());
                    details
                });
                return Err(context.fail(event, failure.failure, &headers));
            }
        };
        let mut terminal: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return Err(context
                    .adapter_failure(event, crate::AdapterError::upstream_response_invalid()))
            }
        };
        event.tool_use.observe_upstream_response(&terminal);
        context.tool_use.upstream_tool_call_count = event.tool_use.upstream_tool_call_count;
        event.tool_use.tool_call_count = 0;
        event.tool_use.text_output = false;
        event.tool_use.finish();
        let continue_lookup = match session.advance(&terminal, &mut upstream) {
            Ok(continue_lookup) => continue_lookup,
            Err(error) => return Err(context.adapter_failure(event, error)),
        };
        session.record(context.tool_use);
        if !continue_lookup {
            session.finish(&mut terminal);
            session.remember(context.runtime, &context.key.id, &terminal);
            // Keep the final generation's actual counters. Earlier lookup
            // usage has its own charged attempt and explicit diagnostics.
            let bytes = serde_json::to_vec(&terminal).map_err(|_| {
                context.adapter_failure(
                    event.clone(),
                    crate::AdapterError::upstream_response_invalid(),
                )
            })?;
            let mut completed = Response::new(Bytes::from(bytes));
            *completed.status_mut() = status;
            *completed.headers_mut() = headers;
            for name in [
                header::CONTENT_LENGTH,
                header::TRANSFER_ENCODING,
                header::CONTENT_ENCODING,
            ] {
                completed.headers_mut().remove(name);
            }
            completed.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
            return Ok(reqwest::Response::from(completed).into());
        }
        record_lookup(context.runtime, &session, context.tool_use, &mut event);
        if !context.budget.can_dispatch() {
            return Err(context.adapter_failure(
                event,
                super::super::response::invalid_tool_output("output.relay.catalog.budget"),
            ));
        }
        emit_usage(context.runtime, event.clone());
        if session.is_native() {
            super::native::capture_wire_catalog(context.tool_use, &upstream);
        }
        context
            .budget
            .retain_input_bytes(crate::gateway::request_body::retained_request_bytes(
                &upstream,
            ));
        context.tool_use.capture_upstream_context(&upstream);
        if let Some(transport) = context
            .route
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.request_payload = None;
        }
        let before = context.budget.dispatches();
        response = match dispatch_continuation(
            context.runtime,
            context.lease,
            context.route,
            context.budget,
            &expected_owner,
            &upstream,
        )
        .await
        {
            Ok(response) => response.response,
            Err(error) => {
                let failure = AttemptFailure::authorized_request(error);
                if context.budget.dispatches() != before {
                    event = context.event(failure.status);
                }
                return Err(context.fail(event, failure, &Default::default()));
            }
        };
    }
}

pub(in crate::gateway) async fn dispatch_continuation(
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    route: &ExecutorRoute,
    budget: &SharedRequestBudget,
    owner: &crate::runtime::AuthorizationIncarnation,
    upstream: &Value,
) -> Result<AuthorizedResponse, crate::runtime::AuthorizedRequestError> {
    let basis_points =
        route.account_transport == crate::runtime::AccountTransport::ExcelBasisPoints;
    runtime.observe_basis_points_dispatch(route, upstream);
    let body = serde_json::to_vec(upstream).expect("prepared JSON is serializable");
    let payload = route.diagnostics_enabled.then(|| {
        crate::usage::RequestPayloadDiagnostics::identity(
            &body,
            if basis_points {
                crate::usage::TransportMode::BasisPoints
            } else {
                crate::usage::TransportMode::Native
            },
        )
    });
    let request = runtime
        .request_client(&route.candidate_id)
        .post(route.upstream_url.clone())
        .headers(route.upstream_headers.clone())
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::ACCEPT,
            if upstream["stream"] == true {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(body);
    let mut response = runtime
        .send_catalog_continuation(
            route,
            request,
            AuthorizationDispatch {
                client_version: None,
                identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                turn_scope: None,
                budget: Some(budget),
                lease: Some(lease),
                usage_event: None,
            },
            owner,
        )
        .await?;
    // Catalog continuations are not compressed or rewritten by Codex guards.
    if let Some(payload) = payload {
        response.response.extensions_mut().insert(payload);
    }
    Ok(response)
}

pub(in crate::gateway) fn record_lookup(
    runtime: &GatewayRuntime,
    session: &DiscoverySession,
    tool_use: &mut ToolUseDiagnostics,
    event: &mut UsageEvent,
) {
    if let Some(catalog) = tool_use.basis_points_catalog.as_mut() {
        catalog.discovery_latency_ms = Some(event.latency_ms);
        catalog.discovery_input_tokens = add_reported(
            catalog.discovery_input_tokens,
            event.input_tokens,
            session.rounds,
        );
        catalog.discovery_output_tokens = add_reported(
            catalog.discovery_output_tokens,
            event.output_tokens,
            session.rounds,
        );
    }
    event.tool_use = tool_use.clone();
    event.tool_use.tool_call_count = 0;
    event.tool_use.finish();
    if let Some(checkpoint) = session.preload_checkpoint {
        runtime.basis_points_catalog_store.remember_checkpoint(
            checkpoint,
            &session.loaded,
            session
                .prompt_prefix
                .as_ref()
                .map(|(scope, text)| (scope, text.as_str())),
            crate::gateway::now_ms(),
        );
    }
    event.success = false;
    if let Some(routing) = event.routing.as_mut() {
        routing.attempt_outcome = Some(crate::AttemptOutcome::ToolDiscovery);
    }
}

fn add_reported(total: Option<u64>, observed: Option<u64>, round: u8) -> Option<u64> {
    if round == 1 {
        observed
    } else {
        total
            .zip(observed)
            .and_then(|(total, observed)| total.checked_add(observed))
    }
}

impl DiscoveryExecution<'_> {
    fn event(&self, status: StatusCode) -> UsageEvent {
        usage_event(
            UsageAttempt {
                request_id: self.request_id,
                attempt: u16::from(self.budget.dispatches()),
                local_key_id: &self.key.id,
                route: self.route,
                reasoning_effort: Some(self.reasoning_effort),
                requested_model: self.requested_model,
                tool_use: self.tool_use.clone(),
            },
            false,
            status.as_u16(),
            None,
            self.started.elapsed().as_millis() as u64,
        )
    }

    fn fail(
        &self,
        mut event: UsageEvent,
        failure: AttemptFailure,
        headers: &axum::http::HeaderMap,
    ) -> Response<Body> {
        event.success = false;
        event.http_status = failure.status.as_u16();
        event.error_category = Some(failure.category.to_string());
        apply_failure_state(
            &mut event,
            settle_route_failure(self.runtime, self.lease, self.route, &failure, headers),
        );
        if let Some(routing) = event.routing.as_mut() {
            routing.attempt_outcome = Some(
                if failure.execution.certainty
                    == crate::scheduler::rotation::ExecutionCertainty::Unknown
                {
                    crate::AttemptOutcome::ExecutionUncertain
                } else {
                    crate::AttemptOutcome::ReturnedFailure
                },
            );
        }
        emit_usage(self.runtime, event);
        attempt_error_response(failure, None, self.selected_error_origin, self.request_id)
    }

    fn adapter_failure(&self, event: UsageEvent, error: crate::AdapterError) -> Response<Body> {
        let mut event = crate::gateway::execution::request::mark_adapter_failure(event, &error);
        if let Some(routing) = event.routing.as_mut() {
            routing.attempt_outcome = Some(crate::AttemptOutcome::ReturnedFailure);
        }
        self.lease
            .settle_rotation_terminal(crate::gateway::now_ms());
        emit_usage(self.runtime, event);
        let origin = match error.parameter() {
            Some(
                "output.relay.catalog.budget"
                | "output.relay.catalog.bytes"
                | "output.relay.catalog.limit",
            ) => ErrorOrigin::Relay,
            _ => self.selected_error_origin,
        };
        adapter_error_response_for_origin(error, origin, Some(self.request_id))
    }
}

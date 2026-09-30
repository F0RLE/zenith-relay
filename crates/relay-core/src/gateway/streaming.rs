use super::errors::{
    api_error_type, apply_failure_state, canonical_upstream_status, current_failure_state,
    failure_category_requires_cooldown, failure_cooldown, preserved_upstream_error_value,
    rate_limit_body_hint_value, upstream_event_failure_category, upstream_failure_status,
    upstream_status_from_value, AttemptFailure, CooldownInput, PreservedUpstreamError,
    RateLimitBodyHint,
};
use super::now_ms;
use super::request::response_tool_call_ids;
use super::response::{
    apply_usage, attach_stream_diagnostics, emit_callback, emit_usage, find_usage,
    proxy_sse_response, response_id, response_service_tier, route_error_origin, usage_event,
    CompletionCallback, UsageAttempt,
};
use crate::error_codes;
use crate::runtime::{CandidateLease, ExecutorRoute};
use crate::usage::ReasoningEffortDiagnostics;
use crate::{
    AdapterStreamBridge, GatewayRuntime, MessagesBridgeResponse, MessagesStreamBridge,
    PreparedAdapterRequest, ToolUseDiagnostics, UsageEvent, WireApi,
};
use axum::body::{Body, Bytes};
use axum::http::{Response, StatusCode};
use futures_util::{stream, Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};
use tokio::time::{sleep, Instant as TokioInstant, Sleep};

pub(super) const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

const SSE_HEARTBEAT: &[u8] = b": keep-alive\n\n";

pub(super) type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

mod bridge;
mod completion;
mod diagnostics;
mod events;
mod replay;
mod upstream_usage;

use bridge::{bridge_adapter_stream, bridge_gemini_stream, bridge_messages_stream};

pub(super) use replay::NativeReplayCapture;

pub(super) use events::{
    has_output_delta, has_semantic_output, is_compaction_payload, is_empty_responses_incomplete,
    is_known_non_output_event, parse_sse_event, preserved_stream_error, rewrite_bridge_failure,
    TerminalEvent, TerminalOutcome,
};

mod bootstrap;
pub(super) use bootstrap::{bootstrap_stream, StreamBootstrapFailure};

/// Owns the work after an upstream stream has emitted client-visible output.
/// From this point the response is committed and no fallback is legal.
pub(in crate::gateway) struct StreamExecution {
    pub(in crate::gateway) runtime: Arc<GatewayRuntime>,
    pub(in crate::gateway) route: ExecutorRoute,
    pub(in crate::gateway) lease: CandidateLease,
    pub(in crate::gateway) adapter_request: PreparedAdapterRequest,
    pub(in crate::gateway) request: Value,
    pub(in crate::gateway) request_id: String,
    pub(in crate::gateway) local_key_id: String,
    pub(in crate::gateway) requested_model: String,
    pub(in crate::gateway) source_model: String,
    pub(in crate::gateway) prompt_affinity_key: Option<String>,
    pub(in crate::gateway) wire_api: WireApi,
    pub(in crate::gateway) reasoning_effort: ReasoningEffortDiagnostics,
    pub(in crate::gateway) tool_use: ToolUseDiagnostics,
    pub(in crate::gateway) attempt: u16,
    pub(in crate::gateway) started: Instant,
}

impl StreamExecution {
    pub(in crate::gateway) fn into_response(
        self,
        status: StatusCode,
        headers: reqwest::header::HeaderMap,
        first: Bytes,
        remaining: UpstreamStream,
    ) -> Response<Body> {
        let Self {
            runtime,
            route,
            lease,
            adapter_request,
            request,
            request_id,
            local_key_id,
            requested_model,
            source_model,
            prompt_affinity_key,
            wire_api,
            reasoning_effort,
            tool_use,
            attempt,
            started,
        } = self;
        let adapter_is_passthrough = adapter_request.is_passthrough();
        let initial_event = usage_event(
            UsageAttempt {
                request_id: &request_id,
                attempt,
                local_key_id: &local_key_id,
                route: &route,
                reasoning_effort: Some(&reasoning_effort),
                requested_model: &requested_model,
                tool_use,
            },
            true,
            status.as_u16(),
            None,
            0,
        );
        let upstream_usage = (!adapter_is_passthrough).then(|| {
            let mut capture = upstream_usage::UpstreamUsage::new(initial_event.clone());
            capture.observe(&first);
            Arc::new(Mutex::new(capture))
        });
        let capture_stream = upstream_usage.clone();
        let remaining: UpstreamStream = Box::pin(remaining.inspect(move |chunk| {
            if let (Some(capture), Ok(bytes)) = (&capture_stream, chunk) {
                capture
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observe(bytes);
            }
        }));
        let completion_runtime = runtime.clone();
        let completion_source = route.candidate_id.clone();
        let completion_model = source_model.clone();
        let completion_prompt_affinity = prompt_affinity_key.clone();
        let completion_headers = headers.clone();
        let completion_uses_response_affinity = wire_api == WireApi::Responses;
        let completion_bridge_state = adapter_request
            .uses_messages_continuation()
            .then(|| Arc::new(Mutex::new(None::<MessagesBridgeResponse>)));
        let completion_bridge_state_for_callback = completion_bridge_state.clone();
        let completion_native_response = (wire_api == WireApi::Responses && adapter_is_passthrough)
            .then(|| Arc::new(Mutex::new(None::<Value>)));
        let completion_native_response_for_callback = completion_native_response.clone();
        let completion_native_template = request;
        let completion_local_key = local_key_id.clone();
        let completion_settlement = completion::StreamCompletionSettlement {
            upstream_usage,
            lease,
            runtime: completion_runtime,
            source: completion_source,
            model: completion_model,
            headers: completion_headers,
            prompt_affinity: completion_prompt_affinity,
            uses_response_affinity: completion_uses_response_affinity,
            bridge_state: completion_bridge_state_for_callback,
            native_response: completion_native_response_for_callback,
            native_template: completion_native_template,
            local_key: completion_local_key,
        };
        let completion: CompletionCallback = Arc::new(move |event, response_id, hint| {
            completion_settlement.settle(event, response_id, hint);
        });
        let combined: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> =
            match adapter_request.into_stream_bridge() {
                Some(AdapterStreamBridge::Messages(bridge)) => {
                    let completed = completion_bridge_state
                        .expect("message bridge state is configured for message routes");
                    Box::pin(bridge_messages_stream(first, remaining, *bridge, completed))
                }
                Some(AdapterStreamBridge::Gemini(bridge)) => {
                    let completed = completion_bridge_state
                        .expect("Gemini bridge state is configured for Gemini routes");
                    Box::pin(bridge_gemini_stream(first, remaining, *bridge, completed))
                }
                Some(bridge @ AdapterStreamBridge::Translated(_)) => {
                    let completed = completion_bridge_state
                        .expect("translation completion state is configured");
                    Box::pin(bridge_adapter_stream(first, remaining, bridge, completed))
                }
                None => Box::pin(
                    stream::once(async move { Ok::<_, reqwest::Error>(first) }).chain(remaining),
                ),
            };
        let usage_stream = UsageStream::with_runtime(
            combined,
            runtime,
            initial_event,
            started,
            completion,
            completion_native_response,
        );
        let origin = route_error_origin(&route);
        let mut response = proxy_sse_response(status, &headers, Body::from_stream(usage_stream));
        attach_stream_diagnostics(&mut response, origin, &request_id);
        response
    }
}

mod usage_stream;

use usage_stream::UsageStream;

#[cfg(test)]
mod tests;

use super::errors::{
    api_error_type, apply_failure_state, canonical_upstream_status, current_failure_state,
    failure_category_requires_cooldown, failure_cooldown, preserved_upstream_error_value,
    rate_limit_body_hint_value, responses_tool_call_links_rejected_value,
    upstream_event_failure_category, upstream_failure_status, upstream_status_from_value,
    zenith_gateway_invalid_request_value, AttemptFailure, PreservedUpstreamError,
    RateLimitBodyHint,
};
use super::now_ms;
use super::request::response_tool_call_ids;
use super::response::{
    apply_usage, attach_stream_diagnostics, emit_callback, emit_usage, find_usage,
    proxy_sse_response, response_id, response_service_tier, route_error_origin, usage_event,
    CompletionCallback,
};
use crate::error_codes;
use crate::protocol::sse_event_end;
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

type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

mod diagnostics;
mod events;
mod replay;
mod upstream_usage;

pub(super) use replay::NativeReplayCapture;

pub(super) use events::{
    has_output_delta, has_semantic_output, is_compaction_payload, is_empty_responses_incomplete,
    is_known_non_output_event, parse_sse_event, preserved_stream_error, rewrite_bridge_failure,
    TerminalEvent, TerminalOutcome,
};

pub(super) struct StreamBootstrapFailure {
    pub(super) execution: crate::scheduler::rotation::ExecutionObservation,
    pub(super) upstream_error: Option<crate::usage::UpstreamErrorDetails>,
    pub(super) failure: AttemptFailure,
    pub(super) preserved: Option<PreservedUpstreamError>,
    pub(super) zenith_gateway_invalid_request: bool,
    pub(super) responses_tool_call_links_rejected: bool,
}

impl From<AttemptFailure> for StreamBootstrapFailure {
    fn from(failure: AttemptFailure) -> Self {
        Self {
            execution: crate::scheduler::rotation::ExecutionObservation::unknown(),
            failure,
            upstream_error: None,
            preserved: None,
            zenith_gateway_invalid_request: false,
            responses_tool_call_links_rejected: false,
        }
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The bounded bootstrap failure carries the diagnostics needed for retry and response ownership."
)]
pub(super) async fn bootstrap_stream(
    upstream: reqwest::Response,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), StreamBootstrapFailure> {
    let headers = upstream.headers().clone();
    let mut stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    let mut buffered = Vec::new();
    let mut inspected = 0;
    let mut saw_output = false;
    let mut completed_output_items = 0_usize;
    // `response.created` and other setup frames do not make a response safe to
    // commit. Keep them private until the source produces real output or a
    // terminal event, so a pre-output provider failure can use another route.
    // No local generation deadline: quiet reasoning or provider queuing is not
    // a failed attempt. EOF, explicit provider errors and cancellation still end it.
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if chunk.len() > MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream(error_codes::STREAM_EVENT_TOO_LARGE).into());
                }
                // Bootstrap may contain a large Responses setup event before the
                // first visible delta. Keep the same bounded budget as the
                // regular SSE parser instead of rejecting valid upstream data
                // at the old 256 KiB bootstrap threshold.
                if buffered.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream(error_codes::STREAM_EVENT_TOO_LARGE).into());
                }
                buffered.extend_from_slice(&chunk);
                let mut ready_to_forward = false;
                while let Some(end) = sse_event_end(&buffered[inspected..]) {
                    let absolute_end = inspected + end;
                    let event = parse_sse_event(&buffered[inspected..absolute_end]);
                    if event.has_data && !event.valid {
                        return Err(StreamBootstrapFailure {
                            upstream_error: event.upstream_error,
                            ..AttemptFailure::stream(error_codes::STREAM_INVALID).into()
                        });
                    }
                    if event.outcome == Some(TerminalOutcome::Failure) {
                        let category = event
                            .error_category
                            .unwrap_or(error_codes::UPSTREAM_TERMINAL);
                        let failure = AttemptFailure::classified_with_hint(
                            event
                                .error_status
                                .unwrap_or_else(|| upstream_failure_status(category)),
                            category,
                            event.cooldown_hint,
                        );
                        return Err(StreamBootstrapFailure {
                            execution: if saw_output {
                                crate::scheduler::rotation::ExecutionObservation::accepted()
                            } else {
                                failure.execution
                            },
                            failure,
                            upstream_error: event.upstream_error,
                            preserved: event.preserved_error,
                            zenith_gateway_invalid_request: event
                                .payload
                                .as_ref()
                                .is_some_and(zenith_gateway_invalid_request_value),
                            responses_tool_call_links_rejected: event
                                .payload
                                .as_ref()
                                .is_some_and(responses_tool_call_links_rejected_value),
                        });
                    }
                    if event.output_item.is_some() && !event.is_compaction {
                        completed_output_items = completed_output_items.saturating_add(1);
                    }
                    saw_output |= event.semantic_output;
                    // A zero-token incomplete response has not committed any
                    // client-visible output. Treat it as a pre-output source
                    // failure, allowing the request executor to retry another
                    // candidate. A non-empty incomplete response remains a
                    // terminal client response (for example max output).
                    if event.payload.as_ref().is_some_and(|payload| {
                        is_empty_responses_incomplete(payload, saw_output, completed_output_items)
                    }) {
                        return Err(AttemptFailure::stream(error_codes::STREAM_INCOMPLETE).into());
                    }
                    let terminal = event.outcome.is_some();
                    ready_to_forward |= terminal || event.semantic_output;
                    inspected = absolute_end;
                    if terminal {
                        // A transport chunk may also contain later frames. The
                        // first terminal owns the response; never expose its tail.
                        buffered.truncate(inspected);
                        break;
                    }
                }
                if ready_to_forward {
                    return Ok((headers, Bytes::from(buffered), stream));
                }
            }
            Some(Err(error)) => return Err(AttemptFailure::transport(&error).into()),
            None => return Err(AttemptFailure::stream(error_codes::STREAM_INCOMPLETE).into()),
        }
    }
}

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
            &request_id,
            attempt,
            &local_key_id,
            &route,
            Some(&reasoning_effort),
            &requested_model,
            true,
            status.as_u16(),
            None,
            0,
            tool_use,
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
        let completion: CompletionCallback = Arc::new(move |event, response_id, hint| {
            if let Some(capture) = &upstream_usage {
                capture
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .apply_to(event);
            }
            if event.success {
                lease.settle_rotation_success(now_ms());
            } else {
                // This callback belongs to an already returned response body.
                // A terminal failure may affect health, but can never replay it.
                let health = if event.error_category.as_deref().is_some_and(|category| {
                    matches!(
                        category,
                        error_codes::UPSTREAM_SERVER_ERROR
                            | error_codes::UPSTREAM_OVERLOADED
                            | error_codes::UPSTREAM_UNAVAILABLE
                    )
                }) {
                    crate::scheduler::rotation::HealthObservation::CountableTransient {
                        provider_not_before_ms: None,
                    }
                } else {
                    crate::scheduler::rotation::HealthObservation::Unknown
                };
                let now = std::time::SystemTime::now();
                let cooldown = event.error_category.as_deref().and_then(|category| {
                    failure_cooldown(
                        &completion_runtime,
                        &completion_source,
                        &completion_model,
                        StatusCode::from_u16(event.http_status).unwrap_or(StatusCode::BAD_GATEWAY),
                        category,
                        &completion_headers,
                        hint,
                        now,
                    )
                });
                completion_runtime.settle_rotation_failure(
                    &lease,
                    crate::scheduler::rotation::AttemptObservation {
                        execution: crate::scheduler::rotation::ExecutionObservation::committed(),
                        health,
                    },
                    cooldown,
                    crate::unix_time_ms_at(now),
                );
            }
            // A response is healthy only after the upstream has emitted its
            // successful terminal event. An incomplete response may have
            // delivered bytes to the client, but it must not warm affinity or
            // reset the selected slot's failure state.
            if event.success {
                let recovered = completion_runtime.record_success_with_metrics(
                    &completion_source,
                    &completion_model,
                    now_ms(),
                    event.output_tokens,
                    event.generation_ms.unwrap_or(event.latency_ms),
                );
                event.consecutive_failures = recovered.then_some(0);
                completion_runtime.bind_prompt_affinity(
                    completion_prompt_affinity.as_deref(),
                    &completion_source,
                    now_ms(),
                );
                if completion_uses_response_affinity {
                    completion_runtime.bind_response_affinity(
                        response_id,
                        &completion_source,
                        now_ms(),
                    );
                }
                if let Some(shared) = completion_bridge_state_for_callback.as_ref() {
                    if let Some(response) = shared
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        completion_runtime.save_messages_bridge_response(
                            &completion_local_key,
                            &completion_source,
                            &response,
                            now_ms(),
                        );
                    }
                }
                if let Some(shared) = completion_native_response_for_callback.as_ref() {
                    if let Some(response) = shared
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        for call_id in response_tool_call_ids(&response) {
                            completion_runtime.bind_tool_call_affinity(
                                &completion_local_key,
                                &call_id,
                                &completion_source,
                                now_ms(),
                            );
                        }
                        completion_runtime.capture_native_responses_replay(
                            &completion_local_key,
                            &completion_source,
                            &completion_native_template,
                            &completion_model,
                            &response,
                            now_ms(),
                        );
                    }
                }
            } else if event
                .error_category
                .as_deref()
                .is_some_and(failure_category_requires_cooldown)
            {
                let state = current_failure_state(
                    &completion_runtime,
                    &completion_source,
                    &completion_model,
                );
                apply_failure_state(event, state);
            }
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

struct AdapterBridgeStreamState {
    inner: UpstreamStream,
    bridge: AdapterStreamBridge,
    pending: VecDeque<Bytes>,
    finished: bool,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
}

/// Translates a native Messages SSE stream into the client-facing Responses
/// SSE contract. The completed bridge response is published before the
/// `response.completed` frame is yielded so the usage callback can persist the
/// continuation without exposing native content outside the local bridge.
pub(super) fn bridge_messages_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: MessagesStreamBridge,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    bridge_adapter_stream(
        first,
        remaining,
        AdapterStreamBridge::Messages(Box::new(bridge)),
        completed,
    )
}

fn bridge_adapter_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: AdapterStreamBridge,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    let inner = stream::once(async move { Ok::<Bytes, reqwest::Error>(first) }).chain(remaining);
    stream::unfold(
        AdapterBridgeStreamState {
            inner: Box::pin(inner),
            bridge,
            pending: VecDeque::new(),
            finished: false,
            completed,
        },
        |mut state| async move {
            loop {
                if let Some(bytes) = state.pending.pop_front() {
                    return Some((Ok(bytes), state));
                }
                if state.finished {
                    return None;
                }

                let mut preserved_error = None;
                match state.inner.next().await {
                    Some(Ok(bytes)) => {
                        state.bridge.push(&bytes);
                        preserved_error = state
                            .bridge
                            .take_upstream_error()
                            .and_then(|error| preserved_stream_error(&error));
                    }
                    Some(Err(_)) | None => {
                        state.bridge.finish();
                        state.finished = true;
                    }
                }

                queue_bridge_output(
                    &mut state.pending,
                    || state.bridge.pop_output(),
                    preserved_error.as_ref(),
                );
                if let Some(response) = state.bridge.completed().cloned() {
                    *state
                        .completed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(response);
                }
                if state.bridge.is_terminal() {
                    state.finished = true;
                }
            }
        },
    )
}

pub(super) fn bridge_gemini_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: crate::GeminiStreamBridge,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    bridge_adapter_stream(
        first,
        remaining,
        AdapterStreamBridge::Gemini(Box::new(bridge)),
        completed,
    )
}

fn queue_bridge_output(
    pending: &mut VecDeque<Bytes>,
    mut next_output: impl FnMut() -> Option<Vec<u8>>,
    error: Option<&PreservedUpstreamError>,
) {
    while let Some(bytes) = next_output() {
        pending.push_back(Bytes::from(rewrite_bridge_failure(bytes, error)));
    }
}

pub(super) struct UsageStream<S> {
    pub(super) inner: Pin<Box<S>>,
    pub(super) runtime: Option<Arc<GatewayRuntime>>,
    pub(super) callback: crate::UsageCallback,
    pub(super) completion: CompletionCallback,
    pub(super) event: Option<UsageEvent>,
    pub(super) response_id: Option<String>,
    pub(super) native_response: Option<Arc<Mutex<Option<Value>>>>,
    pub(super) native_gemini: bool,
    native_gemini_incomplete: bool,
    native_gemini_finished: bool,
    native_replay_capture: NativeReplayCapture,
    pub(super) cooldown_hint: RateLimitBodyHint,
    pub(super) started: Instant,
    pub(super) sse_pending: Vec<u8>,
    pub(super) output_pending: VecDeque<Bytes>,
    // Track yielded bytes, not parsed deltas: even an incomplete SSE frame is
    // already owned by the client and cannot be replaced by a synthetic response.
    client_visible_output: bool,
    pub(super) heartbeat: Pin<Box<Sleep>>,
    pub(super) terminated: bool,
}

impl<S> UsageStream<S> {
    #[cfg(test)]
    pub(super) fn new(
        stream: S,
        callback: crate::UsageCallback,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
    ) -> Self {
        let native_gemini = event.wire_api == WireApi::Gemini;
        Self {
            inner: Box::pin(stream),
            runtime: None,
            callback,
            completion,
            event: Some(event),
            response_id: None,
            native_response: None,
            native_gemini,
            native_gemini_incomplete: false,
            native_gemini_finished: false,
            native_replay_capture: NativeReplayCapture::default(),
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            client_visible_output: false,
            heartbeat: Box::pin(sleep(SSE_HEARTBEAT_INTERVAL)),
            terminated: false,
        }
    }

    pub(super) fn with_runtime(
        stream: S,
        runtime: Arc<GatewayRuntime>,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
        native_response: Option<Arc<Mutex<Option<Value>>>>,
    ) -> Self {
        let callback = runtime.usage.clone();
        let native_gemini = event.wire_api == WireApi::Gemini;
        Self {
            inner: Box::pin(stream),
            runtime: Some(runtime),
            callback,
            completion,
            event: Some(event),
            response_id: None,
            native_response,
            native_gemini,
            native_gemini_incomplete: false,
            native_gemini_finished: false,
            native_replay_capture: NativeReplayCapture::default(),
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            client_visible_output: false,
            heartbeat: Box::pin(sleep(SSE_HEARTBEAT_INTERVAL)),
            terminated: false,
        }
    }

    fn finish(&mut self, success: Option<bool>, category: Option<&str>) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        if let Some(success) = success {
            event.success = success;
        }
        if let Some(category) = category {
            event.error_category = Some(category.to_string());
        }
        if event.success {
            event.tool_use.finish();
        }
        if !event.success
            && event.http_status < 400
            && event.error_category.as_deref() != Some(error_codes::RESPONSE_INCOMPLETE)
        {
            event.http_status = event
                .error_category
                .as_deref()
                .filter(|category| *category != error_codes::CLIENT_CANCELLED)
                .map(upstream_failure_status)
                .unwrap_or(StatusCode::BAD_GATEWAY)
                .as_u16();
        }
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        event.generation_ms = event
            .ttft_ms
            .map(|ttft_ms| event.latency_ms.saturating_sub(ttft_ms))
            .filter(|duration| *duration > 0);
        (self.completion)(&mut event, self.response_id.as_deref(), self.cooldown_hint);
        if let Some(runtime) = self.runtime.as_deref() {
            emit_usage(runtime, event);
        } else {
            emit_callback(&self.callback, event);
        }
    }

    fn queue_responses_failure(&mut self, category: &str) -> bool {
        let Some(event) = self.event.as_ref() else {
            return false;
        };
        if event.wire_api != WireApi::Responses || self.client_visible_output {
            return false;
        }
        let response_id = self.response_id.clone().unwrap_or_else(|| {
            let suffix = event
                .request_id
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>();
            format!("resp_{suffix}")
        });
        let message = match category {
            error_codes::STREAM_INVALID => "Upstream returned an invalid streaming event",
            error_codes::STREAM_EVENT_TOO_LARGE => {
                "Upstream streaming event exceeded the size limit"
            }
            error_codes::STREAM_INCOMPLETE => "Upstream stream ended before response.completed",
            _ => "Upstream stream disconnected before completion",
        };
        let payload = json!({
            "type": "response.failed",
            "response": {
                "id": response_id,
                "object": "response",
                "model": event.requested_model.clone().unwrap_or_default(),
                "status": "failed",
                "output": [],
                "error": {
                    "type": error_codes::STREAM_ERROR,
                    "code": category,
                    "message": message,
                    "zenith_relay": {
                        "origin": Self::stream_error_origin(event).as_str(),
                        "category": category,
                        "request_id": &event.request_id,
                    },
                }
            }
        });
        let Ok(payload) = serde_json::to_vec(&payload) else {
            return false;
        };
        let mut frame = Vec::with_capacity(payload.len() + 44);
        frame.extend_from_slice(b"event: response.failed\ndata: ");
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(b"\n\n");
        self.output_pending.push_back(Bytes::from(frame));
        true
    }

    fn stream_error_origin(event: &UsageEvent) -> crate::ErrorOrigin {
        if event.account_id.is_some() {
            crate::ErrorOrigin::Account
        } else {
            crate::ErrorOrigin::Provider
        }
    }

    fn fail_stream(&mut self, category: &str) -> bool {
        let framed = self.queue_responses_failure(category);
        self.finish(Some(false), Some(category));
        self.terminated = true;
        framed
    }

    fn ingest_sse(&mut self, bytes: &[u8]) -> (bool, usize) {
        if self.terminated {
            return (false, 0);
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
            return (false, 0);
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.sse_pending) {
            if end > MAX_SSE_EVENT_BYTES {
                self.sse_pending.clear();
                self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
                return (false, 0);
            }
            let event = self.sse_pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                self.set_upstream_error(terminal.upstream_error);
                self.sse_pending.clear();
                self.fail_stream(error_codes::STREAM_INVALID);
                return (false, 0);
            }
            // A terminal marker from another wire protocol cannot prove this
            // request or its continuation state completed successfully.
            let valid_terminal = self
                .event
                .as_ref()
                .is_some_and(|event| match event.wire_api {
                    WireApi::Responses => terminal.payload.as_ref().is_some_and(|payload| {
                        matches!(
                            payload.get("type").and_then(Value::as_str),
                            Some("response.completed" | "response.done")
                        )
                    }),
                    WireApi::ChatCompletions => terminal.payload.is_none(),
                    WireApi::Messages => terminal.payload.as_ref().is_some_and(|payload| {
                        payload.get("type").and_then(Value::as_str) == Some("message_stop")
                    }),
                    WireApi::Gemini => false,
                });
            if terminal.outcome == Some(TerminalOutcome::Success) && !valid_terminal {
                self.sse_pending.clear();
                self.fail_stream(error_codes::STREAM_INCOMPLETE);
                return (false, 0);
            }
            if let Some(payload) = terminal.payload.as_ref() {
                if let Some(current) = self.event.as_mut() {
                    current.tool_use.observe_stream_payload(payload);
                }
                if self.native_response.is_some() {
                    self.native_replay_capture.observe(payload);
                }
            } else if terminal.is_compaction {
                self.native_replay_capture.mark_unmaterialized();
            }
            if terminal.has_output_delta
                && self
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none())
            {
                if let Some(current) = self.event.as_mut() {
                    current.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
                }
            }
            if let Some(usage) = terminal.usage {
                if let Some(current) = self.event.as_mut() {
                    apply_usage(current, &usage);
                }
            }
            if let Some(service_tier) = terminal.applied_service_tier {
                if let Some(current) = self.event.as_mut() {
                    current.applied_service_tier = Some(service_tier);
                }
            }
            if terminal.response_id.is_some() {
                self.response_id = terminal.response_id;
            }
            match terminal.outcome {
                Some(TerminalOutcome::Success) => {
                    self.capture_native_response(terminal.response);
                    self.finish(None, None);
                    self.terminated = true;
                }
                Some(TerminalOutcome::Incomplete) => {
                    self.capture_native_response(terminal.response);
                    self.finish(
                        Some(false),
                        Some(
                            terminal
                                .error_category
                                .unwrap_or(error_codes::RESPONSE_INCOMPLETE),
                        ),
                    );
                    self.terminated = true;
                }
                Some(TerminalOutcome::Failure) => {
                    self.cooldown_hint = terminal.cooldown_hint;
                    self.set_upstream_error(terminal.upstream_error);
                    self.finish(
                        Some(false),
                        Some(
                            terminal
                                .error_category
                                .unwrap_or(error_codes::UPSTREAM_TERMINAL),
                        ),
                    );
                    self.terminated = true;
                }
                None => {}
            }
            if self.terminated {
                let forward_len = bytes.len().saturating_sub(self.sse_pending.len());
                self.sse_pending.clear();
                return (true, forward_len);
            }
        }
        if self.sse_pending.len() > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
            return (false, 0);
        }
        (true, bytes.len())
    }

    /// Gemini's native SSE ends at EOF rather than with `response.completed`.
    /// Require a recognized final candidate or prompt block before treating
    /// that EOF as a completed request; keep the provider bytes untouched.
    fn ingest_native_gemini(&mut self, bytes: &[u8]) -> bool {
        if self.terminated {
            return false;
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream(error_codes::STREAM_EVENT_TOO_LARGE);
            return false;
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.sse_pending) {
            let event = self.sse_pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                self.set_upstream_error(terminal.upstream_error);
                self.fail_stream(error_codes::STREAM_INVALID);
                self.terminated = true;
                return false;
            }
            if terminal.outcome == Some(TerminalOutcome::Failure) {
                self.cooldown_hint = terminal.cooldown_hint;
                self.set_upstream_error(terminal.upstream_error);
                self.finish(
                    Some(false),
                    Some(
                        terminal
                            .error_category
                            .unwrap_or(error_codes::UPSTREAM_TERMINAL),
                    ),
                );
                self.terminated = true;
                return true;
            }
            if terminal.outcome == Some(TerminalOutcome::Incomplete) {
                self.native_gemini_incomplete = true;
            }
            // Gemini has no generic [DONE] or Responses-style terminal event.
            // Only a candidate's own finish reason proves a completed generation.
            if terminal.payload.as_ref().is_some_and(|payload| {
                payload
                    .pointer("/candidates/0/finishReason")
                    .and_then(Value::as_str)
                    == Some("STOP")
            }) {
                self.native_gemini_finished = true;
            }
            if let Some(usage) = terminal.usage {
                if let Some(current) = self.event.as_mut() {
                    apply_usage(current, &usage);
                }
            }
            if terminal.has_output_delta
                && self
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none())
            {
                if let Some(current) = self.event.as_mut() {
                    current.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
                }
            }
        }
        true
    }

    fn set_upstream_error(&mut self, details: Option<crate::usage::UpstreamErrorDetails>) {
        if let Some(current) = self.event.as_mut() {
            current.upstream_error = details.map(|mut details| {
                details.http_status = Some(current.http_status);
                details
            });
        }
    }

    fn capture_native_response(&mut self, response: Option<Value>) {
        let Some(shared) = self.native_response.as_ref() else {
            return;
        };
        let Some(response) = std::mem::take(&mut self.native_replay_capture)
            .finish(response, self.response_id.as_deref())
        else {
            return;
        };
        *shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(response);
    }
}

impl<S, E> Stream for UsageStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, E>>,
{
    type Item = std::result::Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if let Some(bytes) = this.output_pending.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }
            if this.terminated {
                return Poll::Ready(None);
            }
            match this.inner.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(bytes))) => {
                    let now = TokioInstant::now();
                    this.heartbeat.as_mut().reset(now + SSE_HEARTBEAT_INTERVAL);
                    let (valid, forward_len) = if this.native_gemini {
                        (this.ingest_native_gemini(&bytes), bytes.len())
                    } else {
                        this.ingest_sse(&bytes)
                    };
                    if let Some(failure) = this.output_pending.pop_front() {
                        return Poll::Ready(Some(Ok(failure)));
                    }
                    if !valid {
                        return Poll::Ready(None);
                    }
                    let forwarded = bytes.slice(..forward_len);
                    this.client_visible_output |= !forwarded.is_empty();
                    if !forwarded.is_empty() {
                        return Poll::Ready(Some(Ok(forwarded)));
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    if this.fail_stream(error_codes::UPSTREAM_STREAM) {
                        continue;
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if this.native_gemini {
                        if !this.sse_pending.is_empty() {
                            this.fail_stream(error_codes::STREAM_INCOMPLETE);
                        } else if this.native_gemini_incomplete {
                            this.finish(Some(false), Some(error_codes::RESPONSE_INCOMPLETE));
                        } else if this.native_gemini_finished {
                            this.finish(Some(true), None);
                        } else {
                            this.fail_stream(error_codes::STREAM_INCOMPLETE);
                        }
                        this.sse_pending.clear();
                        this.terminated = true;
                        return Poll::Ready(None);
                    }
                    if this.event.as_ref().is_some_and(|event| event.success) {
                        if this.fail_stream(error_codes::STREAM_INCOMPLETE) {
                            continue;
                        }
                    } else {
                        this.finish(None, None);
                    }
                    this.sse_pending.clear();
                    this.terminated = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    // Keep the client connection alive without imposing a
                    // deadline on the provider's next output.
                    // Chunks are forwarded immediately, so a heartbeat is safe
                    // only between complete SSE events, never inside a frame.
                    if this.sse_pending.is_empty()
                        && this.heartbeat.as_mut().poll(context).is_ready()
                    {
                        this.heartbeat
                            .as_mut()
                            .reset(TokioInstant::now() + SSE_HEARTBEAT_INTERVAL);
                        return Poll::Ready(Some(Ok(Bytes::from_static(SSE_HEARTBEAT))));
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

impl<S> Drop for UsageStream<S> {
    fn drop(&mut self) {
        self.finish(Some(false), Some(error_codes::CLIENT_CANCELLED));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::test_support::test_usage_event;
    use std::convert::Infallible;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn usage_stream_with_events<S>(input: S, events: Arc<Mutex<Vec<UsageEvent>>>) -> UsageStream<S>
    where
        S: Stream<Item = Result<Bytes, Infallible>>,
    {
        let captured = events.clone();
        UsageStream::new(
            input,
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        )
    }

    async fn response_from_sse_event(
        event: String,
    ) -> (reqwest::Response, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            event.len(), event
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await;
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let upstream = reqwest::get(format!("http://{address}/stream"))
            .await
            .unwrap();
        (upstream, server)
    }

    #[test]
    fn streaming_terminal_errors_keep_the_canonical_category() {
        let terminal = parse_sse_event(
            br#"data: {"type":"response.failed","response":{"error":{"type":"usage_limit_reached","resets_in_seconds":7}}}

"#,
        );
        assert_eq!(terminal.error_category, Some("upstream_quota_exhausted"));
        assert_eq!(terminal.error_status, Some(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(terminal.cooldown_hint.retry_after_ms, Some(7_000));
        assert!(terminal.cooldown_hint.global);
    }

    #[test]
    fn generic_gateway_rejection_sse_keeps_candidate_category_and_provider_details() {
        let terminal = parse_sse_event(
            br#"event: error
data: {"type":"error","error":{"type":"invalid_request_error","code":"invalid_request","message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}

"#,
        );

        assert_eq!(terminal.error_category, Some("upstream_candidate_rejected"));
        assert_eq!(terminal.error_status, Some(StatusCode::SERVICE_UNAVAILABLE));
        let upstream = terminal.upstream_error.unwrap();
        assert_eq!(upstream.code.as_deref(), Some("invalid_request"));
        assert_eq!(
            upstream.error_type.as_deref(),
            Some("invalid_request_error")
        );
        assert_eq!(upstream.http_status, None);
    }

    #[tokio::test]
    async fn bootstrap_retries_empty_zero_token_incomplete_without_committing_output() {
        let event = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"output\":[],\"usage\":{\"output_tokens\":0}}}\n\n"
        );
        let (upstream, server) = response_from_sse_event(event.into()).await;
        let failure = bootstrap_stream(upstream)
            .await
            .err()
            .expect("empty incomplete stream must not commit client output");
        server.await.unwrap();

        assert_eq!(failure.failure.category, "stream_incomplete");
        assert_eq!(failure.failure.status, StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn bootstrap_does_not_commit_an_opaque_compaction_before_disconnect() {
        let event = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"compaction\",\"encrypted_content\":\"opaque\"}}\n\n"
        );
        let (upstream, server) = response_from_sse_event(event.into()).await;
        let failure = bootstrap_stream(upstream)
            .await
            .err()
            .expect("compaction alone must remain retryable");
        server.await.unwrap();

        assert_eq!(failure.failure.category, "stream_incomplete");
        assert_eq!(failure.failure.status, StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn large_valid_bootstrap_event_is_not_rejected_at_the_old_limit() {
        let delta = "a".repeat(300 * 1024);
        let event =
            format!("data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{delta}\"}}\n\n");
        let (upstream, server) = response_from_sse_event(event).await;
        let result = bootstrap_stream(upstream).await;
        server.await.unwrap();
        assert!(
            result.is_ok(),
            "valid large Responses event should bootstrap"
        );
        let (_, buffered, _) = if let Ok(value) = result {
            value
        } else {
            return;
        };
        assert!(buffered.len() > 256 * 1024);
        assert!(buffered.starts_with(b"data: {\"type\":\"response.output_text.delta\""));
    }

    #[tokio::test]
    async fn oversized_sse_event_is_recorded_as_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            UsageEvent {
                request_id: "request".into(),
                attempt: 1,
                local_key_id: "key".into(),
                source_id: "source".into(),
                candidate_id: Some("source".into()),
                account_id: None,
                account_token_generation: None,
                client_context_id: None,
                routing: None,
                requested_model: Some("model".into()),
                resolved_model: Some("model".into()),
                requested_reasoning_effort: None,
                effective_reasoning_effort: None,
                wire_api: crate::WireApi::Responses,
                service_tier: crate::DefaultServiceTier::Standard,
                applied_service_tier: None,
                success: true,
                http_status: 200,
                error_category: None,
                tool_use: crate::ToolUseDiagnostics::default(),
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: Some(0),
                latency_ms: 0,
                ttft_ms: None,
                generation_ms: None,
                input_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                cache_write_ttl: None,
                reasoning_tokens: None,
                output_tokens: None,
                total_tokens: None,
                upstream_error: None,
                quota_snapshot: None,
            },
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.ingest_sse(&vec![b'x'; MAX_SSE_EVENT_BYTES + 1]);
        assert!(stream.terminated);
        assert!(stream.sse_pending.is_empty());
        let failure =
            String::from_utf8(stream.output_pending.pop_front().unwrap().to_vec()).unwrap();
        assert!(failure.starts_with("event: response.failed\ndata: "));
        let payload = failure
            .strip_prefix("event: response.failed\ndata: ")
            .and_then(|value| value.strip_suffix("\n\n"))
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
            .unwrap();
        assert_eq!(
            payload["response"]["error"]["code"],
            "stream_event_too_large"
        );
        assert_eq!(
            payload["response"]["error"]["zenith_relay"]["origin"],
            "provider"
        );
        assert_eq!(
            payload["response"]["error"]["zenith_relay"]["category"],
            "stream_event_too_large"
        );
        assert_eq!(
            payload["response"]["error"]["zenith_relay"]["request_id"],
            "request"
        );
        assert!(stream.output_pending.is_empty());
        drop(stream);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("stream_event_too_large")
        );
    }

    #[tokio::test]
    async fn heartbeat_never_splits_an_unfinished_sse_frame() {
        let frame =
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\r\n\r\n";
        for native_gemini in [false, true] {
            for split in 1..frame.len() {
                let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                let input = stream::unfold(receiver, |mut receiver| async move {
                    receiver.recv().await.map(|chunk| (chunk, receiver))
                });
                let mut stream = usage_stream_with_events(input, Arc::default());
                stream.native_gemini = native_gemini;
                sender
                    .send(Ok(Bytes::copy_from_slice(&frame[..split])))
                    .unwrap();
                let first = stream.next().await.unwrap().unwrap();
                stream
                    .heartbeat
                    .as_mut()
                    .reset(TokioInstant::now() - Duration::from_secs(1));
                let pending = futures_util::future::poll_fn(|context| {
                    Poll::Ready(Pin::new(&mut stream).poll_next(context))
                })
                .await;
                if split == frame.len() - 1 {
                    // CR already completes the blank line; its optional LF
                    // can arrive after a heartbeat without changing the data.
                    assert_eq!(
                        pending,
                        Poll::Ready(Some(Ok(Bytes::from_static(SSE_HEARTBEAT))))
                    );
                    assert!(parse_sse_event(&frame[..split]).valid);
                } else {
                    assert!(pending.is_pending(), "heartbeat inserted at byte {split}");
                }
                sender
                    .send(Ok(Bytes::copy_from_slice(&frame[split..])))
                    .unwrap();
                let last = stream.next().await.unwrap().unwrap();
                assert_eq!([first.as_ref(), last.as_ref()].concat(), frame);
                stream
                    .heartbeat
                    .as_mut()
                    .reset(TokioInstant::now() - Duration::from_secs(1));
                assert_eq!(stream.next().await.unwrap().unwrap(), SSE_HEARTBEAT);
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_stream_keeps_sending_heartbeats_until_provider_completion() {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let input = stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|chunk| (chunk, receiver))
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut stream = usage_stream_with_events(input, events.clone());
        let first = Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
        );
        sender.send(Ok(first.clone())).unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap(), first);

        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(20 * 60)).await;
            assert_eq!(stream.next().await.unwrap().unwrap(), SSE_HEARTBEAT);
            assert!(events.lock().unwrap().is_empty());
            assert!(!stream.terminated);
        }

        let completed = Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"slow-response\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );
        sender.send(Ok(completed.clone())).unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap(), completed);
        assert!(stream.next().await.is_none());
        drop(stream);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert_eq!(events[0].total_tokens, Some(3));
        assert_eq!(events[0].cached_input_tokens, None);
    }

    #[tokio::test]
    async fn usage_stream_forwards_chunks_without_waiting_for_an_sse_boundary() {
        let first =
            Bytes::from_static(br#"data: {"type":"response.output_text.delta","delta":"hel"#);
        let second = Bytes::from_static(b"lo\"}\n\n");
        let completed = Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\"}}\n\n",
        );
        let input = futures_util::stream::iter([
            Ok::<_, Infallible>(first.clone()),
            Ok(second.clone()),
            Ok(completed.clone()),
        ]);
        let mut stream = UsageStream::new(
            input,
            Arc::new(|_| {}),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );

        assert_eq!(stream.next().await.unwrap().unwrap(), first);
        assert_eq!(stream.next().await.unwrap().unwrap(), second);
        assert_eq!(stream.next().await.unwrap().unwrap(), completed);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn usage_stream_does_not_append_a_synthetic_failure_after_visible_bytes() {
        let partial = [
            Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            ),
            Bytes::from_static(
                b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
            ),
            Bytes::from_static(
                b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}\n\n",
            ),
            Bytes::from_static(br#"data: {"type":"response.output_text.delta","delta":"par"#),
        ];
        let cases = partial
            .into_iter()
            .map(|first| (vec![first], "stream_incomplete"))
            .chain(std::iter::once((
                vec![
                    Bytes::from_static(
                        b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
                    ),
                    Bytes::from_static(b"data: invalid-json\n\n"),
                ],
                "stream_invalid",
            )));
        for (input, category) in cases {
            let first = input[0].clone();
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut stream = usage_stream_with_events(
                stream::iter(input.into_iter().map(Ok::<Bytes, Infallible>)),
                events.clone(),
            );

            assert_eq!(stream.next().await.unwrap().unwrap(), first);
            assert!(stream.next().await.is_none());
            drop(stream);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert!(!events[0].success);
            assert_eq!(events[0].error_category.as_deref(), Some(category));
            if category == error_codes::STREAM_INVALID {
                let details = events[0].upstream_error.as_ref().unwrap();
                assert_eq!(details.error_type.as_deref(), Some("relay_stream_parser"));
                assert!(!details.message.as_ref().unwrap().contains("invalid-json"));
            }
        }
    }

    #[tokio::test]
    async fn usage_stream_preserves_transport_errors_after_partial_output() {
        let first = Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        );
        let mut stream = UsageStream::new(
            stream::iter([
                Ok(first.clone()),
                Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
            ]),
            Arc::new(|_| {}),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );

        assert_eq!(stream.next().await.unwrap().unwrap(), first);
        assert_eq!(
            stream.next().await.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::ConnectionReset
        );
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn usage_stream_still_reports_a_failure_before_any_visible_bytes() {
        let mut stream = UsageStream::new(
            stream::empty::<Result<Bytes, Infallible>>(),
            Arc::new(|_| {}),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );

        let bytes = stream.next().await.unwrap().unwrap();
        let failure = parse_sse_event(&bytes);
        assert_eq!(failure.outcome, Some(TerminalOutcome::Failure));
        assert_eq!(
            failure.payload.unwrap()["response"]["error"]["code"],
            "stream_incomplete"
        );
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn usage_stream_preserves_upstream_terminal_failures_after_output() {
        let first = Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        );
        let failure = Bytes::from_static(
            b"data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_test\",\"error\":{\"code\":\"server_error\"}}}\n\n",
        );
        let mut stream = UsageStream::new(
            stream::iter([Ok::<_, Infallible>(first.clone()), Ok(failure.clone())]),
            Arc::new(|_| {}),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );

        assert_eq!(stream.next().await.unwrap().unwrap(), first);
        assert_eq!(stream.next().await.unwrap().unwrap(), failure);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn native_responses_stream_capture_keeps_completed_tool_output_for_http_replay() {
        let captured = Arc::new(Mutex::new(None));
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(|_| {}),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.native_response = Some(captured.clone());
        stream.ingest_sse(
            b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_stream_01\",\"name\":\"run_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
        );
        stream.ingest_sse(
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_stream_01\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );

        let response = captured
            .lock()
            .unwrap()
            .clone()
            .expect("completed native stream is captured");
        assert_eq!(response["id"], "resp_stream_01");
        assert_eq!(response["output"][0]["type"], "function_call");
        assert_eq!(response["output"][0]["call_id"], "call_stream_01");
    }

    #[tokio::test]
    async fn incomplete_native_response_is_captured_without_gateway_failure_status() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = events.clone();
        let captured_response = Arc::new(Mutex::new(None));
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured_events.lock().unwrap().push(event)),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.native_response = Some(captured_response.clone());
        stream.ingest_sse(
            br#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[],"usage":{"input_tokens":3,"output_tokens":4}}}

"#,
        );

        let response = captured_response
            .lock()
            .unwrap()
            .clone()
            .expect("incomplete native stream is captured");
        assert_eq!(response["id"], "resp_incomplete");
        assert_eq!(
            response["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(events[0].http_status, StatusCode::OK.as_u16());
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("response_incomplete")
        );
        assert_eq!(events[0].output_tokens, Some(4));
    }

    #[tokio::test]
    async fn streaming_chat_usage_captures_cached_prompt_tokens() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.ingest_sse(
            b"data: {\"type\":\"response.completed\",\"response\":{\"service_tier\":\"default\",\"usage\":{\"prompt_tokens\":32,\"prompt_tokens_details\":{\"cached_tokens\":9,\"cache_write_tokens\":7},\"completion_tokens\":6,\"completion_tokens_details\":{\"reasoning_tokens\":4}}}}\n\n",
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(32));
        assert_eq!(events[0].cached_input_tokens, Some(9));
        assert_eq!(events[0].cache_write_input_tokens, Some(7));
        assert_eq!(events[0].reasoning_tokens, Some(4));
        assert_eq!(events[0].output_tokens, Some(6));
        assert_eq!(events[0].total_tokens, Some(38));
        assert_eq!(events[0].applied_service_tier, Some("default".to_string()));
    }

    #[test]
    fn response_incomplete_is_a_terminal_non_failure_outcome() {
        let event = parse_sse_event(
            br#"data: {"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}

"#,
        );
        assert_eq!(event.outcome, Some(TerminalOutcome::Incomplete));
        assert_eq!(event.error_category, Some("response_incomplete"));
    }

    #[test]
    fn all_responses_error_terminal_types_are_failures() {
        for event_type in [
            "response.failed",
            "response.cancelled",
            "response.canceled",
            "error",
        ] {
            let event = format!("data: {{\"type\":\"{event_type}\"}}\n\n");
            assert_eq!(
                parse_sse_event(event.as_bytes()).outcome,
                Some(TerminalOutcome::Failure)
            );
        }
    }

    #[test]
    fn bridge_failure_rewrite_preserves_the_upstream_type_and_event_name() {
        let preserved = PreservedUpstreamError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "upstream_unavailable",
            code: "service_unavailable".into(),
            message: "safe upstream message".into(),
            error_type: None,
        };
        let rewritten = String::from_utf8(rewrite_bridge_failure(
            br#"event: response.cancelled
data: {"type":"response.cancelled","response":{"error":{"type":"invalid_request_error","code":"adapter_upstream_stream_invalid","message":"adapter message"}}}

"#
            .to_vec(),
            Some(&preserved),
        ))
        .unwrap();

        assert!(rewritten.starts_with("event: response.cancelled\ndata: "));
        assert!(rewritten.contains("\"type\":\"server_error\""));
        assert!(rewritten.contains("\"code\":\"service_unavailable\""));
        assert!(rewritten.contains("\"message\":\"safe upstream message\""));
    }

    type EmptyGeminiUsageStream =
        UsageStream<futures_util::stream::Empty<Result<Bytes, Infallible>>>;

    fn native_gemini_test_stream() -> (EmptyGeminiUsageStream, Arc<Mutex<Vec<UsageEvent>>>) {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let captured = recorded.clone();
        let mut event = test_usage_event();
        event.wire_api = WireApi::Gemini;
        let stream = UsageStream::new(
            futures_util::stream::empty::<Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            event,
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        (stream, recorded)
    }

    #[tokio::test]
    async fn native_responses_done_marker_without_terminal_is_not_success() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let input = stream::iter([Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"))]);
        let mut stream = usage_stream_with_events(input, events.clone());
        let failure = stream.next().await.unwrap().unwrap();
        let terminal = parse_sse_event(&failure);
        assert_eq!(terminal.outcome, Some(TerminalOutcome::Failure));
        assert_eq!(
            terminal.payload.as_ref().unwrap()["response"]["error"]["code"],
            error_codes::STREAM_INCOMPLETE
        );
        assert!(stream.next().await.is_none());
        drop(stream);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some(error_codes::STREAM_INCOMPLETE)
        );
    }

    #[tokio::test]
    async fn foreign_protocol_terminal_markers_never_complete_a_stream() {
        for (wire_api, marker) in [
            (
                WireApi::Responses,
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_slice(),
            ),
            (
                WireApi::Messages,
                b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
            ),
            (
                WireApi::ChatCompletions,
                b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
            ),
        ] {
            let recorded = Arc::new(Mutex::new(Vec::new()));
            let captured = recorded.clone();
            let mut event = test_usage_event();
            event.wire_api = wire_api;
            let input = stream::iter([Ok::<_, Infallible>(Bytes::copy_from_slice(marker))]);
            let mut stream = UsageStream::new(
                input,
                Arc::new(move |event| captured.lock().unwrap().push(event)),
                event,
                Instant::now(),
                Arc::new(|_, _, _| {}),
            );
            while stream.next().await.is_some() {}
            let events = recorded.lock().unwrap();
            assert_eq!(events.len(), 1, "{wire_api:?}");
            assert!(!events[0].success, "{wire_api:?}");
            assert_eq!(
                events[0].error_category.as_deref(),
                Some(error_codes::STREAM_INCOMPLETE),
                "{wire_api:?}"
            );
        }
    }

    #[tokio::test]
    async fn native_gemini_error_is_not_promoted_to_success_at_eof() {
        let (mut stream, recorded) = native_gemini_test_stream();
        assert!(stream.ingest_native_gemini(b"data: {\"error\":{\"code\":400,\"status\":\"INVALID_ARGUMENT\",\"message\":\"Invalid field: temperature\"}}\n\n"));
        assert!(stream.terminated);
        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(!recorded[0].success);
        let details = recorded[0].upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(200));
        assert_eq!(details.code.as_deref(), Some("400"));
        assert_eq!(
            details.message.as_deref(),
            Some("Invalid field: temperature")
        );
    }

    #[tokio::test]
    async fn native_gemini_rejects_malformed_frame_after_valid_output() {
        let (mut stream, recorded) = native_gemini_test_stream();
        assert!(stream.ingest_native_gemini(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n"
        ));
        assert!(!stream.ingest_native_gemini(b"data: {broken\n\n"));
        assert!(stream.terminated);
        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(!recorded[0].success);
        assert_eq!(
            recorded[0].error_category.as_deref(),
            Some("stream_invalid")
        );
    }

    #[tokio::test]
    async fn native_gemini_foreign_terminal_markers_do_not_complete_generation() {
        for marker in [
            b"data: [DONE]\n\n".as_slice(),
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        ] {
            let (mut stream, recorded) = native_gemini_test_stream();
            assert!(stream.ingest_native_gemini(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n"
            ));
            assert!(stream.ingest_native_gemini(marker));
            assert!(stream.next().await.is_none());
            let events = recorded.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert!(!events[0].success);
            assert_eq!(
                events[0].error_category.as_deref(),
                Some("stream_incomplete")
            );
        }
    }

    #[test]
    fn bridge_rewrite_keeps_unknown_provider_codes_and_error_types() {
        let value = json!({"error": {"code": "future_constraint", "type": "future_provider_type", "message": "Constraint check failed"}});
        let preserved = preserved_stream_error(&value).unwrap();
        let bytes = rewrite_bridge_failure(b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"adapter_upstream_stream_invalid\"}}}\n\n".to_vec(), Some(&preserved));
        let details = parse_sse_event(&bytes).upstream_error.unwrap();
        assert_eq!(details.code.as_deref(), Some("future_constraint"));
        assert_eq!(details.error_type.as_deref(), Some("future_provider_type"));
        assert_eq!(details.message.as_deref(), Some("Constraint check failed"));
    }

    #[test]
    fn ttft_requires_real_output_for_supported_stream_protocols() {
        for event in [
            "data: {\"type\":\"response.created\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null}}\n\n",
        ] {
            assert!(!parse_sse_event(event.as_bytes()).has_output_delta);
        }
        for event in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"thinking\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"summary\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"PowerShell\"}}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}]}}]}\n\n",
        ] {
            assert!(parse_sse_event(event.as_bytes()).has_output_delta);
        }
    }

    #[test]
    fn streamed_custom_tool_input_commits_the_response() {
        let event = parse_sse_event(
            br#"data: {"type":"response.custom_tool_call_input.delta","delta":"{"}

"#,
        );

        assert!(event.has_output_delta);
    }
}

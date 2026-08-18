use super::errors::{
    api_error_type, apply_failure_cooldown_with_hint, apply_failure_state,
    canonical_upstream_status, failure_category_requires_cooldown, preserved_upstream_error_value,
    rate_limit_body_hint_value, upstream_event_failure_category, upstream_failure_status,
    upstream_status_from_value, zenith_gateway_invalid_request_value, AttemptFailure,
    CooldownContext, PreservedUpstreamError, RateLimitBodyHint,
};
use super::now_ms;
use super::response::{
    apply_usage, attach_stream_diagnostics, emit_callback, emit_usage, find_usage,
    proxy_sse_response, response_id, response_service_tier, route_error_origin, usage_event,
    CompletionCallback,
};
use crate::protocol::sse_event_end;
use crate::runtime::{CandidateLease, DefaultServiceTier, ExecutorRoute};
use crate::usage::ReasoningEffortDiagnostics;
use crate::{
    AdapterStreamBridge, GatewayRuntime, MessagesBridgeResponse, MessagesStreamBridge,
    NativeResponsesReplayState, PreparedAdapterRequest, ToolUseDiagnostics, UsageEvent, WireApi,
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

const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

const MAX_REPLAY_BOOTSTRAP_BYTES: usize = 256 * 1024;

const SSE_FIRST_BYTE_TIMEOUT: Duration = SSE_IDLE_TIMEOUT;

const SSE_REPLAY_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(2);

const SSE_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

const SSE_HEARTBEAT: &[u8] = b": keep-alive\n\n";

type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

mod events;

pub(super) use events::{
    has_output_delta, parse_sse_event, preserved_stream_error, rewrite_bridge_failure,
    TerminalOutcome,
};

pub(super) struct StreamBootstrapFailure {
    pub(super) failure: AttemptFailure,
    pub(super) preserved: Option<PreservedUpstreamError>,
    pub(super) zenith_gateway_invalid_request: bool,
}

impl From<AttemptFailure> for StreamBootstrapFailure {
    fn from(failure: AttemptFailure) -> Self {
        Self {
            failure,
            preserved: None,
            zenith_gateway_invalid_request: false,
        }
    }
}

pub(super) async fn bootstrap_stream(
    upstream: reqwest::Response,
    wait_for_native_replay_error: bool,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), StreamBootstrapFailure> {
    let headers = upstream.headers().clone();
    let mut stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    let mut buffered = Vec::new();
    let mut inspected = 0;
    let mut first_chunk = true;
    let mut replay_probe_deadline = None;

    loop {
        let timeout = if first_chunk {
            SSE_FIRST_BYTE_TIMEOUT
        } else {
            let replay_probe_deadline = replay_probe_deadline
                .expect("replay probe deadline is set after the first stream chunk");
            let now = TokioInstant::now();
            if now >= replay_probe_deadline {
                return Ok((headers, Bytes::from(buffered), stream));
            }
            replay_probe_deadline - now
        };
        match tokio::time::timeout(timeout, stream.next()).await {
            Err(_) if wait_for_native_replay_error && !buffered.is_empty() => {
                return Ok((headers, Bytes::from(buffered), stream));
            }
            Err(_) => return Err(AttemptFailure::stream("stream_first_byte_timeout").into()),
            Ok(Some(Ok(chunk))) => {
                if chunk.len() > MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream("stream_event_too_large").into());
                }
                buffered.extend_from_slice(&chunk);
                let mut ready_to_forward = false;
                while let Some(end) = sse_event_end(&buffered[inspected..]) {
                    let absolute_end = inspected + end;
                    let event = parse_sse_event(&buffered[inspected..absolute_end]);
                    if event.has_data && !event.valid {
                        return Err(AttemptFailure::stream("stream_invalid").into());
                    }
                    if event.outcome == Some(TerminalOutcome::Failure) {
                        let category = event.error_category.unwrap_or("upstream_terminal");
                        let failure = AttemptFailure::classified_with_hint(
                            event
                                .error_status
                                .unwrap_or_else(|| upstream_failure_status(category)),
                            category,
                            event.cooldown_hint,
                        );
                        return Err(StreamBootstrapFailure {
                            failure,
                            preserved: event.preserved_error,
                            zenith_gateway_invalid_request: event
                                .payload
                                .as_ref()
                                .is_some_and(zenith_gateway_invalid_request_value),
                        });
                    }
                    ready_to_forward |= event.outcome.is_some()
                        || event.has_output_delta
                        || event.output_item.is_some();
                    inspected = absolute_end;
                }
                if !wait_for_native_replay_error
                    || ready_to_forward
                    || buffered.len() >= MAX_REPLAY_BOOTSTRAP_BYTES
                {
                    return Ok((headers, Bytes::from(buffered), stream));
                }
                if first_chunk {
                    first_chunk = false;
                    replay_probe_deadline =
                        Some(TokioInstant::now() + SSE_REPLAY_BOOTSTRAP_TIMEOUT);
                }
            }
            Ok(Some(Err(error))) => return Err(AttemptFailure::transport(&error).into()),
            Ok(None) => return Err(AttemptFailure::stream("stream_incomplete").into()),
        }
    }
}

/// Owns the work that starts after an upstream stream has emitted safe first
/// bytes. From this point the response is committed and no fallback is legal.
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
        let completion_runtime = runtime.clone();
        let completion_source = route.candidate_id.clone();
        let completion_model = source_model.clone();
        let completion_prompt_affinity = prompt_affinity_key.clone();
        let completion_half_open_probe = route.half_open_probe;
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
        let completion_scope = route.scope.clone();
        let completion_allowed_protocols = route.allowed_protocols.clone();
        let completion: CompletionCallback = Arc::new(move |event, response_id, hint| {
            lease.release();
            let response_delivered =
                event.success || event.error_category.as_deref() == Some("response_incomplete");
            if response_delivered {
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
                        if let Some((response_id, replay)) =
                            NativeResponsesReplayState::from_response(
                                &completion_native_template,
                                &completion_model,
                                &response,
                            )
                        {
                            completion_runtime.save_native_responses_replay(
                                &completion_local_key,
                                &completion_source,
                                &response_id,
                                replay,
                                now_ms(),
                            );
                        }
                    }
                }
            } else if let Some(category) = event
                .error_category
                .as_deref()
                .filter(|category| failure_category_requires_cooldown(category))
            {
                let status =
                    StatusCode::from_u16(event.http_status).unwrap_or(StatusCode::BAD_GATEWAY);
                let cooldown_context = CooldownContext {
                    scope: &completion_scope,
                    allowed_protocols: &completion_allowed_protocols,
                };
                let state = apply_failure_cooldown_with_hint(
                    &completion_runtime,
                    &completion_source,
                    &completion_model,
                    status,
                    category,
                    &completion_headers,
                    hint,
                    &cooldown_context,
                    completion_half_open_probe,
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
                    Box::pin(bridge_gemini_stream(first, remaining, *bridge))
                }
                None => Box::pin(
                    stream::once(async move { Ok::<_, reqwest::Error>(first) }).chain(remaining),
                ),
            };
        let usage_stream = UsageStream::with_runtime(
            combined,
            runtime,
            usage_event(
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
            ),
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

struct MessagesBridgeStreamState {
    inner: UpstreamStream,
    bridge: MessagesStreamBridge,
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
    let inner = stream::once(async move { Ok::<Bytes, reqwest::Error>(first) }).chain(remaining);
    stream::unfold(
        MessagesBridgeStreamState {
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

                while let Some(bytes) = state.bridge.pop_output() {
                    state.pending.push_back(Bytes::from(rewrite_bridge_failure(
                        bytes,
                        preserved_error.as_ref(),
                    )));
                }
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

struct GeminiBridgeStreamState {
    inner: UpstreamStream,
    bridge: crate::GeminiStreamBridge,
    pending: VecDeque<Bytes>,
    finished: bool,
}

pub(super) fn bridge_gemini_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: crate::GeminiStreamBridge,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    let inner = stream::once(async move { Ok::<Bytes, reqwest::Error>(first) }).chain(remaining);
    stream::unfold(
        GeminiBridgeStreamState {
            inner: Box::pin(inner),
            bridge,
            pending: VecDeque::new(),
            finished: false,
        },
        |mut state| async move {
            loop {
                if let Some(bytes) = state.pending.pop_front() {
                    return Some((Ok(bytes), state));
                }
                if state.finished {
                    return None;
                }
                match state.inner.next().await {
                    Some(Ok(bytes)) => state.bridge.push(&bytes),
                    Some(Err(_)) | None => {
                        state.bridge.finish();
                        state.finished = true;
                    }
                }
                while let Some(bytes) = state.bridge.pop_output() {
                    state.pending.push_back(Bytes::from(bytes));
                }
                if state.bridge.is_terminal() {
                    state.finished = true;
                }
            }
        },
    )
}

pub(super) struct UsageStream<S> {
    pub(super) inner: Pin<Box<S>>,
    pub(super) runtime: Option<Arc<GatewayRuntime>>,
    pub(super) callback: crate::UsageCallback,
    pub(super) completion: CompletionCallback,
    pub(super) event: Option<UsageEvent>,
    pub(super) response_id: Option<String>,
    pub(super) native_response: Option<Arc<Mutex<Option<Value>>>>,
    pub(super) native_output_items: Vec<Value>,
    pub(super) cooldown_hint: RateLimitBodyHint,
    pub(super) started: Instant,
    pub(super) sse_pending: Vec<u8>,
    pub(super) output_pending: VecDeque<Bytes>,
    pub(super) heartbeat: Pin<Box<Sleep>>,
    pub(super) idle_watchdog: Pin<Box<Sleep>>,
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
        Self {
            inner: Box::pin(stream),
            runtime: None,
            callback,
            completion,
            event: Some(event),
            response_id: None,
            native_response: None,
            native_output_items: Vec::new(),
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            heartbeat: Box::pin(sleep(SSE_HEARTBEAT_INTERVAL)),
            idle_watchdog: Box::pin(sleep(SSE_IDLE_TIMEOUT)),
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
        Self {
            inner: Box::pin(stream),
            runtime: Some(runtime),
            callback,
            completion,
            event: Some(event),
            response_id: None,
            native_response,
            native_output_items: Vec::new(),
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            heartbeat: Box::pin(sleep(SSE_HEARTBEAT_INTERVAL)),
            idle_watchdog: Box::pin(sleep(SSE_IDLE_TIMEOUT)),
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
            && event.error_category.as_deref() != Some("response_incomplete")
        {
            event.http_status = event
                .error_category
                .as_deref()
                .filter(|category| *category != "client_cancelled")
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
        if event.wire_api != WireApi::Responses {
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
            "stream_invalid" => "Upstream returned an invalid streaming event",
            "stream_event_too_large" => "Upstream streaming event exceeded the size limit",
            "stream_incomplete" => "Upstream stream ended before response.completed",
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
                    "type": "stream_error",
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

    fn ingest_sse(&mut self, bytes: &[u8]) {
        if self.terminated {
            return;
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream("stream_event_too_large");
            return;
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.sse_pending) {
            if end > MAX_SSE_EVENT_BYTES {
                self.sse_pending.clear();
                self.fail_stream("stream_event_too_large");
                return;
            }
            let event = self.sse_pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                self.sse_pending.clear();
                self.fail_stream("stream_invalid");
                return;
            }
            if let Some(payload) = terminal.payload.as_ref() {
                if let Some(current) = self.event.as_mut() {
                    current.tool_use.observe_stream_payload(payload);
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
            if let Some(output_item) = terminal.output_item {
                self.native_output_items.push(output_item);
            }
            match terminal.outcome {
                Some(TerminalOutcome::Success) => {
                    self.capture_native_response(terminal.response);
                    self.finish(None, None);
                    self.terminated = true;
                    return;
                }
                Some(TerminalOutcome::Incomplete) => {
                    self.capture_native_response(terminal.response);
                    self.finish(
                        Some(false),
                        Some(terminal.error_category.unwrap_or("response_incomplete")),
                    );
                    self.terminated = true;
                    return;
                }
                Some(TerminalOutcome::Failure) => {
                    self.cooldown_hint = terminal.cooldown_hint;
                    self.finish(
                        Some(false),
                        Some(terminal.error_category.unwrap_or("upstream_terminal")),
                    );
                    self.terminated = true;
                    return;
                }
                None => {}
            }
        }
        if self.sse_pending.len() > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream("stream_event_too_large");
        }
    }

    fn capture_native_response(&mut self, response: Option<Value>) {
        let Some(shared) = self.native_response.as_ref() else {
            return;
        };
        let mut response = response.unwrap_or_else(|| {
            json!({
                "id": self.response_id,
                "output": self.native_output_items.clone(),
            })
        });
        if let Some(object) = response.as_object_mut() {
            object
                .entry("id")
                .or_insert_with(|| json!(self.response_id));
            let output_is_empty = object
                .get("output")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty);
            if output_is_empty {
                object.insert(
                    "output".to_string(),
                    Value::Array(self.native_output_items.clone()),
                );
            }
        }
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
                    this.idle_watchdog.as_mut().reset(now + SSE_IDLE_TIMEOUT);
                    this.ingest_sse(&bytes);
                    if let Some(failure) = this.output_pending.pop_front() {
                        return Poll::Ready(Some(Ok(failure)));
                    }
                    return Poll::Ready(Some(Ok(bytes)));
                }
                Poll::Ready(Some(Err(error))) => {
                    if this.fail_stream("upstream_stream") {
                        continue;
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if this.event.as_ref().is_some_and(|event| event.success) {
                        if this.fail_stream("stream_incomplete") {
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
                    if this.idle_watchdog.as_mut().poll(context).is_ready() {
                        if this.fail_stream("stream_idle_timeout") {
                            continue;
                        }
                        return Poll::Ready(None);
                    }
                    if this.heartbeat.as_mut().poll(context).is_ready() {
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
        self.finish(Some(false), Some("client_cancelled"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::test_support::test_usage_event;
    use std::convert::Infallible;
    use std::sync::Mutex;

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
    fn delayed_gateway_invalid_request_sse_keeps_request_status() {
        let terminal = parse_sse_event(
            br#"event: error
data: {"type":"error","error":{"type":"invalid_request_error","code":"invalid_request","message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}

"#,
        );

        assert_eq!(terminal.error_category, Some("upstream_invalid_request"));
        assert_eq!(terminal.error_status, Some(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn first_sse_event_gets_the_same_patience_as_an_active_stream() {
        assert_eq!(SSE_FIRST_BYTE_TIMEOUT, SSE_IDLE_TIMEOUT);
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
                routing: None,
                requested_model: Some("model".into()),
                resolved_model: Some("model".into()),
                requested_reasoning_effort: None,
                effective_reasoning_effort: None,
                wire_api: crate::WireApi::Responses,
                service_tier: DefaultServiceTier::Standard,
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
        assert_eq!(
            events[0].applied_service_tier,
            Some(DefaultServiceTier::Standard)
        );
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

    #[test]
    fn ttft_requires_real_output_for_supported_stream_protocols() {
        for event in [
            "data: {\"type\":\"response.created\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"hidden\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null}}\n\n",
        ] {
            assert!(!parse_sse_event(event.as_bytes()).has_output_delta);
        }
        for event in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"PowerShell\"}}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
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

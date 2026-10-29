//! Request-owned discovery between native SSE and the ordinary BPS translator.
//! Text before a withheld tool streams immediately; executable calls never do.

use super::*;
use crate::gateway::errors::{AttemptFailure, RateLimitBodyHint};
use crate::gateway::execution::basis_points::discovery::execution::{
    StreamingDiscovery, dispatch_continuation, record_lookup,
};
use crate::gateway::response::{completed_upstream_response, emit_usage, populate_tokens};
use crate::gateway::streaming::completion::StreamCompletionSettlement;
use crate::{AdapterError, ToolUseDiagnostics, UsageEvent};
use axum::http::{HeaderMap, StatusCode};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

// Keep the same terminal/error/usage validator, without retaining every text
// delta a second time after it has already been forwarded to the client.
fn retain_frame(
    event: &crate::gateway::streaming::TerminalEvent,
    saw_generation: &mut bool,
) -> bool {
    let generated = event.semantic_output
        || event
            .event_payload
            .as_ref()
            .is_some_and(crate::gateway::response::has_generated_tokens);
    let first_generation = generated && !*saw_generation;
    *saw_generation |= generated;
    let ordinary_delta = event.event_payload.as_ref().is_some_and(|payload| {
        matches!(
            payload["type"].as_str(),
            Some(
                "response.output_text.delta"
                    | "response.reasoning_text.delta"
                    | "response.reasoning_summary_text.delta"
            )
        ) && payload.as_object().is_some_and(|fields| {
            fields.keys().all(|field| {
                matches!(
                    field.as_str(),
                    "type"
                        | "sequence_number"
                        | "item_id"
                        | "output_index"
                        | "content_index"
                        | "summary_index"
                        | "delta"
                        | "logprobs"
                )
            })
        })
    });
    // Only known ordinary deltas are redundant. Keep model/tier/usage/error
    // evidence and unknown metadata even when a live guard is disabled now.
    first_generation
        || !event.valid
        || event.outcome.is_some()
        || event.output_item.is_some()
        || event.applied_service_tier.is_some()
        || !ordinary_delta
}

pub(in crate::gateway::streaming) struct DiscoveryProgress {
    pub upstream_error: Option<crate::usage::UpstreamErrorDetails>,
    pub cooldown_hint: RateLimitBodyHint,
    pub budget: crate::scheduler::rotation::SharedRequestBudget,
    pub tool_use: ToolUseDiagnostics,
    pub request_payload: Option<crate::usage::RequestPayloadDiagnostics>,
    pub headers: Option<HeaderMap>,
}

enum DiscoveryFailure {
    Adapter(AdapterError),
    Upstream(AttemptFailure),
}

impl From<AdapterError> for DiscoveryFailure {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

struct DiscoveryStream {
    inner: UpstreamStream,
    discovery: StreamingDiscovery,
    settlement: Arc<StreamCompletionSettlement>,
    progress: Arc<Mutex<DiscoveryProgress>>,
    event: UsageEvent,
    tool_use: ToolUseDiagnostics,
    started: Instant,
    pending: Vec<u8>,
    tools: NativeTools,
    raw: Vec<u8>,
    received_bytes: usize,
    saw_generation: bool,
    output: VecDeque<Bytes>,
    response_id: Option<String>,
    offset: usize,
    seen: HashSet<usize>,
    blocked: bool,
    round_terminal: bool,
    terminal: bool,
    sse: bool,
    status: StatusCode,
    headers: HeaderMap,
    sequence: u64,
}

impl DiscoveryStream {
    fn emit(&mut self, mut event: Value) {
        event["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        if let Some(index) = event.get_mut("output_index")
            && let Some(local) = index.as_u64()
        {
            *index = json!(local + self.offset as u64);
        }
        if let Some(response) = event.get_mut("response")
            && let Some(id) = &self.response_id
        {
            response["id"] = json!(id);
        }
        if event.get("response_id").is_some()
            && let Some(id) = &self.response_id
        {
            event["response_id"] = json!(id);
        }
        self.output.push_back(Bytes::from(format!(
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap_or("error")
        )));
    }

    fn ingest(&mut self, bytes: Bytes) -> Result<(), AdapterError> {
        self.received_bytes = self.received_bytes.saturating_add(bytes.len());
        if self.received_bytes > MAX_RESPONSE_BYTES {
            return Err(
                AdapterError::upstream_stream_invalid().with_parameter("stream.response_size")
            );
        }
        if !self.sse {
            self.raw.extend_from_slice(&bytes);
            return Ok(());
        }
        let mut pending = std::mem::take(&mut self.pending);
        let mut result = Ok(());
        crate::protocol::sse::consume_frames(
            &mut pending,
            &bytes,
            self,
            |state| state.round_terminal,
            |state, frame| {
                if let Err(error) = state.ingest_frame(frame) {
                    result = Err(error);
                    state.round_terminal = true;
                }
            },
        );
        self.pending = pending;
        result
    }

    fn ingest_frame(&mut self, frame: &[u8]) -> Result<(), AdapterError> {
        let data = sse_event_data(frame);
        if data.is_empty() {
            return Ok(());
        }
        let mut parsed = crate::gateway::streaming::parse_sse_event(frame);
        if retain_frame(&parsed, &mut self.saw_generation) {
            self.raw.extend_from_slice(frame);
        }
        let mut event = parsed.event_payload.take().ok_or_else(|| {
            AdapterError::upstream_stream_invalid().with_parameter("stream.event.json")
        })?;
        if event.get("type").is_none() && event["error"].is_object() {
            event["type"] = json!("error");
        }
        let kind = event["type"]
            .as_str()
            .or_else(|| {
                crate::protocol::sse_lines(frame)
                    .find_map(|line| line.strip_prefix(b"event:"))
                    .and_then(|kind| std::str::from_utf8(kind.trim_ascii()).ok())
            })
            .ok_or_else(AdapterError::upstream_stream_invalid)?
            .to_owned();
        event["type"] = json!(kind);
        if matches!(
            kind.as_str(),
            "response.completed"
                | "response.done"
                | "response.incomplete"
                | "response.failed"
                | "response.cancelled"
                | "response.canceled"
                | "error"
        ) || event["error"].is_object()
        {
            self.round_terminal = true;
            return Ok(());
        }
        if matches!(kind.as_str(), "response.created" | "response.in_progress") {
            if self.offset == 0 && self.response_id.is_none() {
                self.response_id = event["response"]["id"].as_str().map(str::to_owned);
                let mut event = event;
                event["type"] = json!(kind);
                event["response"]["output"] = json!([]);
                self.emit(event);
            }
            return Ok(());
        }
        // Removing a hidden lookup can shift later output indices. Do not
        // publish those indices speculatively: flush the suffix at terminal.
        if self
            .tools
            .observe(&event)
            .map_err(|parameter| AdapterError::upstream_stream_invalid().with_parameter(parameter))?
            .is_some()
        {
            self.blocked = true;
        }
        if !self.blocked
            && let Some(index) = event["output_index"].as_u64()
        {
            let index =
                usize::try_from(index).map_err(|_| AdapterError::upstream_stream_invalid())?;
            if index >= MAX_TOOLS {
                return Err(AdapterError::upstream_stream_invalid());
            }
            self.seen.insert(index);
            self.emit(event);
        }
        Ok(())
    }

    fn update_progress(&mut self) {
        let mut progress = crate::poison::mutex(&self.progress);
        progress.tool_use = self.tool_use.clone();
        progress.request_payload = self
            .event
            .routing
            .as_ref()
            .and_then(|routing| routing.transport.as_ref())
            .and_then(|transport| transport.request_payload.clone());
    }

    fn reject(&mut self, error: DiscoveryFailure) {
        self.update_progress();
        self.output.push_back(match error {
            DiscoveryFailure::Adapter(error) => adapter_failure(error),
            DiscoveryFailure::Upstream(error) => {
                crate::poison::mutex(&self.progress).cooldown_hint = error.cooldown_hint;
                let mut event = failure_event(error.category, None);
                event["message"] = json!(error.message);
                failure_frame(event)
            }
        });
        self.terminal = true;
    }

    async fn finish_round(&mut self) -> Result<(), DiscoveryFailure> {
        let mut event = self.event.clone();
        event.attempt = u16::from(self.discovery.budget.dispatches());
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        if !self.sse
            && let Some(capture) = &self.settlement.upstream_usage
        {
            crate::poison::mutex(capture).finish_json_generation(&self.raw);
        }
        let expected = self
            .settlement
            .runtime
            .block_degraded_routes_enabled()
            .then_some(self.settlement.route.source_model.as_str());
        if !self.status.is_success() {
            crate::poison::mutex(&self.progress).upstream_error =
                Some(crate::usage::UpstreamErrorDetails::from_response(
                    Some(self.status.as_u16()),
                    &self.raw,
                    &self.headers,
                ));
            return Err(DiscoveryFailure::Upstream(
                AttemptFailure::status_with_body(self.status, Some(&self.raw))
                    .with_retry_headers(&self.headers),
            ));
        }
        let bytes = completed_upstream_response(&self.raw, true, expected).map_err(|failure| {
            crate::poison::mutex(&self.progress).upstream_error =
                failure.upstream_error.map(|mut details| {
                    details.http_status = Some(self.status.as_u16());
                    details
                });
            DiscoveryFailure::Upstream(failure.failure)
        })?;
        let mut terminal: Value = serde_json::from_slice(&bytes)
            .map_err(|_| AdapterError::upstream_response_invalid())?;
        populate_tokens(&mut event, &self.raw);
        // The parsed terminal now owns the required output. Release retained
        // wire frames before waiting for another lookup round or client poll.
        self.raw = Vec::new();
        if terminal["status"] == "completed" {
            let items = terminal["output"]
                .as_array()
                .ok_or_else(AdapterError::upstream_response_invalid)?;
            self.tools.complete(items).map_err(|parameter| {
                AdapterError::upstream_stream_invalid().with_parameter(parameter)
            })?;
        }
        let next = self
            .discovery
            .session
            .advance(&terminal, &mut self.discovery.upstream)?;
        self.discovery.session.record(&mut self.tool_use);
        if self.response_id.is_none() {
            self.response_id = terminal["id"].as_str().map(str::to_owned);
        }
        if !next {
            self.discovery.session.finish(&mut terminal);
            self.discovery.session.remember(
                &self.settlement.runtime,
                &self.event.local_key_id,
                &terminal,
            );
            if let Some(id) = &self.response_id {
                terminal["id"] = json!(id);
            }
            self.update_progress();
            if self.discovery.session.is_native() {
                // There is no BPS envelope translator after native discovery.
                // Flush withheld client calls only after validating the entire
                // terminal, preserving indexes of text already streamed.
                let seen = (0..self.offset)
                    .chain(self.seen.iter().map(|index| index + self.offset))
                    .collect();
                self.offset = 0;
                emit_unseen_output(&terminal, &seen, |event| self.emit(event));
            }
            // This is the only terminal exposed to BPS validation. Its tools
            // are still withheld by the existing translator until all validate.
            let kind = if terminal["status"] == "incomplete" {
                "response.incomplete"
            } else {
                "response.completed"
            };
            self.offset = 0;
            self.emit(json!({"type":kind,"response":terminal}));
            self.terminal = true;
            return Ok(());
        }
        let items = terminal["output"]
            .as_array_mut()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        items.retain(|item| {
            !matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        });
        let count = items.len();
        let seen = std::mem::take(&mut self.seen);
        emit_unseen_output(&terminal, &seen, |event| self.emit(event));
        self.offset += count;
        record_lookup(
            &self.settlement.runtime,
            &self.discovery.session,
            &mut self.tool_use,
            &mut event,
        );
        if !self.discovery.budget.can_dispatch() {
            return Err(AdapterError::upstream_response_invalid()
                .with_parameter("output.relay.catalog.budget")
                .into());
        }
        emit_usage(&self.settlement.runtime, event);
        if self.discovery.session.is_native() {
            crate::gateway::execution::basis_points::discovery::native::capture_wire_catalog(
                &mut self.tool_use,
                &self.discovery.upstream,
            );
        }
        self.discovery.budget.retain_input_bytes(
            crate::gateway::request_body::retained_request_bytes(&self.discovery.upstream),
        );
        self.tool_use
            .capture_upstream_context(&self.discovery.upstream);
        self.update_progress();
        if let Some(capture) = &self.settlement.upstream_usage {
            crate::poison::mutex(capture).reset_generation();
        }
        if let Some(transport) = self
            .event
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.request_payload = None;
        }
        let response = dispatch_continuation(
            &self.settlement.runtime,
            &self.settlement.lease,
            &self.settlement.route,
            &self.discovery.budget,
            &self.discovery.owner,
            &self.discovery.upstream,
        )
        .await;
        self.update_progress();
        let response = response
            .map_err(|error| DiscoveryFailure::Upstream(AttemptFailure::authorized_request(error)))?
            .response;
        if let Some(transport) = self
            .event
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            transport.request_payload = response
                .extensions()
                .get::<crate::usage::RequestPayloadDiagnostics>()
                .cloned();
        }
        self.update_progress();
        self.status = response.status();
        self.headers = response.headers().clone();
        crate::poison::mutex(&self.progress).headers = Some(self.headers.clone());
        self.sse = self
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|mime| {
                mime.split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
            });
        let capture = self.settlement.upstream_usage.clone();
        let sse = self.sse;
        self.inner = guard_progress(
            Box::pin(
                response
                    .bytes_stream()
                    .map(move |chunk| match (sse, &capture, chunk) {
                        (true, Some(capture), Ok(bytes)) => {
                            Ok(crate::poison::mutex(capture).forward(bytes))
                        }
                        (_, _, chunk) => chunk,
                    }),
            ),
            if self.discovery.session.is_native() {
                None
            } else {
                crate::transport::basis_points_progress_timeout()
            },
        );
        self.raw.clear();
        self.received_bytes = 0;
        self.saw_generation = false;
        self.pending.clear();
        self.blocked = false;
        self.round_terminal = false;
        Ok(())
    }
}

pub(in crate::gateway::streaming) fn translate_stream(
    first: Bytes,
    remaining: UpstreamStream,
    discovery: StreamingDiscovery,
    settlement: Arc<StreamCompletionSettlement>,
    progress: Arc<Mutex<DiscoveryProgress>>,
    event: UsageEvent,
    started: Instant,
) -> UpstreamStream {
    let mut state = DiscoveryStream {
        inner: remaining,
        discovery,
        settlement,
        progress,
        tool_use: event.tool_use.clone(),
        event,
        started,
        pending: Vec::new(),
        tools: NativeTools::default(),
        raw: Vec::new(),
        received_bytes: 0,
        saw_generation: false,
        output: VecDeque::new(),
        response_id: None,
        offset: 0,
        seen: HashSet::new(),
        blocked: false,
        round_terminal: false,
        terminal: false,
        sse: true,
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        sequence: 0,
    };
    if let Err(error) = state.ingest(first) {
        state.reject(error.into());
    }
    Box::pin(stream::unfold(state, |mut state| async move {
        loop {
            if let Some(bytes) = state.output.pop_front() {
                return Some((Ok(bytes), state));
            }
            if state.terminal {
                return None;
            }
            if state.round_terminal {
                if let Err(error) = state.finish_round().await {
                    state.reject(error);
                }
                continue;
            }
            match state.inner.next().await {
                Some(Ok(bytes)) => {
                    if let Err(error) = state.ingest(bytes) {
                        state.reject(error.into());
                    }
                }
                Some(Err(error)) => {
                    state.terminal = true;
                    return Some((Err(error), state));
                }
                None if !state.sse && !state.raw.is_empty() => {
                    state.round_terminal = true;
                }
                None => state.reject(
                    AdapterError::upstream_stream_invalid()
                        .with_parameter("stream.terminal")
                        .into(),
                ),
            }
        }
    }))
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    fn frame(value: Value) -> Vec<u8> {
        format!(
            "event: {}\ndata: {value}\n\n",
            value["type"].as_str().unwrap()
        )
        .into_bytes()
    }

    #[test]
    fn compact_capture_keeps_completed_items_and_usage_without_text_deltas() {
        let mut wire = Vec::new();
        for _ in 0..1_000 {
            wire.extend(frame(
                json!({"type":"response.output_text.delta","output_index":0,"delta":"hello"}),
            ));
        }
        wire.extend(frame(json!({"type":"response.output_item.done","output_index":0,
            "item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}})));
        wire.extend(frame(json!({"type":"response.completed","response":{
            "id":"resp_synthetic","status":"completed","output":[],"usage":{"input_tokens":12,"output_tokens":3}}})));
        let mut retained = Vec::new();
        let mut seen = false;
        let mut offset = 0;
        while let Some(end) = crate::protocol::sse_event_end(&wire[offset..]) {
            let bytes = &wire[offset..offset + end];
            let parsed = crate::gateway::streaming::parse_sse_event(bytes);
            if retain_frame(&parsed, &mut seen) {
                retained.extend_from_slice(bytes);
            }
            offset += end;
        }
        let complete = |bytes: &[u8]| {
            completed_upstream_response(bytes, true, None)
                .map_err(|_| ())
                .expect("Synthetic SSE capture rejected")
        };
        assert_eq!(complete(&retained), complete(&wire));
        assert!(retained.len() * 100 < wire.len());
    }

    #[test]
    fn compact_capture_keeps_failure_generation_and_model_evidence() {
        let delta =
            frame(json!({"type":"response.output_text.delta","output_index":0,"delta":"hello"}));
        let failure =
            frame(json!({"type":"error","code":"server_error","message":"Synthetic failure"}));
        let mut seen = false;
        assert!(retain_frame(
            &crate::gateway::streaming::parse_sse_event(&delta),
            &mut seen
        ));
        assert!(retain_frame(
            &crate::gateway::streaming::parse_sse_event(&failure),
            &mut seen
        ));
        let error =
            completed_upstream_response(&[delta, failure].concat(), true, None).unwrap_err();
        assert_eq!(
            error.execution,
            crate::scheduler::rotation::ExecutionObservation::accepted()
        );
        let model = frame(json!({"type":"response.created","response":{"model":"different"}}));
        assert!(retain_frame(
            &crate::gateway::streaming::parse_sse_event(&model),
            &mut seen
        ));
    }

    #[test]
    fn compact_capture_retains_metadata_and_degraded_evidence_after_generation() {
        let mut seen = true;
        for payload in [
            json!({"type":"response.metadata","service_tier":"priority"}),
            json!({"type":"response.future_metadata","future_field":true}),
            json!({"type":"response.output_text.delta","delta":"x","model":"degrade1"}),
            json!({"type":"response.output_text.delta","delta":"x","service_tier":"priority"}),
            json!({"type":"response.reasoning_text.delta","delta":"x","usage":{"output_tokens":3}}),
            json!({"type":"response.output_text.delta","delta":"x","future_field":true}),
        ] {
            assert!(retain_frame(
                &crate::gateway::streaming::parse_sse_event(&frame(payload)),
                &mut seen
            ));
        }
        for kind in [
            "response.output_text.delta",
            "response.reasoning_text.delta",
            "response.reasoning_summary_text.delta",
        ] {
            assert!(!retain_frame(
                &crate::gateway::streaming::parse_sse_event(&frame(
                    json!({"type":kind,"delta":"x"})
                )),
                &mut seen
            ));
        }
    }
}

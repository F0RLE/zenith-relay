//! Request-owned discovery between native SSE and the ordinary BPS translator.
//! Text before a withheld tool streams immediately; executable calls never do.

use super::*;
use crate::gateway::errors::AttemptFailure;
use crate::gateway::execution::basis_points::discovery::execution::{
    dispatch_continuation, record_lookup, StreamingDiscovery,
};
use crate::gateway::response::{completed_upstream_response, populate_tokens};
use crate::gateway::streaming::completion::StreamCompletionSettlement;
use crate::{AdapterError, ToolUseDiagnostics, UsageEvent};
use axum::http::{HeaderMap, StatusCode};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(in crate::gateway::streaming) struct DiscoveryProgress {
    pub budget: crate::scheduler::rotation::SharedRequestBudget,
    pub tool_use: ToolUseDiagnostics,
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
    raw: Vec<u8>,
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
}

impl DiscoveryStream {
    fn emit(&mut self, mut event: Value) {
        if let Some(index) = event.get_mut("output_index") {
            if let Some(local) = index.as_u64() {
                *index = json!(local + self.offset as u64);
            }
        }
        if let Some(response) = event.get_mut("response") {
            if let Some(id) = &self.response_id {
                response["id"] = json!(id);
            }
        }
        if event.get("response_id").is_some() {
            if let Some(id) = &self.response_id {
                event["response_id"] = json!(id);
            }
        }
        self.output.push_back(Bytes::from(format!(
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap_or("error")
        )));
    }

    fn ingest(&mut self, bytes: Bytes) -> Result<(), AdapterError> {
        if self.raw.len().saturating_add(bytes.len()) > MAX_RESPONSE_BYTES {
            return Err(
                AdapterError::upstream_stream_invalid().with_parameter("stream.response_size")
            );
        }
        self.raw.extend_from_slice(&bytes);
        if !self.sse {
            return Ok(());
        }
        self.pending.extend_from_slice(&bytes);
        while let Some(frame) = take_sse_event(&mut self.pending) {
            let data = sse_event_data(&frame);
            if data.is_empty() {
                continue;
            }
            let mut event: Value = serde_json::from_slice(&data).map_err(|_| {
                AdapterError::upstream_stream_invalid().with_parameter("stream.event.json")
            })?;
            let kind = event["type"]
                .as_str()
                .or_else(|| {
                    crate::protocol::sse_lines(&frame)
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
                self.pending.clear();
                break;
            }
            if matches!(kind.as_str(), "response.created" | "response.in_progress") {
                if self.offset == 0 && self.response_id.is_none() {
                    self.response_id = event["response"]["id"].as_str().map(str::to_owned);
                    let mut event = event;
                    event["type"] = json!(kind);
                    event["response"]["output"] = json!([]);
                    self.emit(event);
                }
                continue;
            }
            // Removing a hidden lookup can shift later output indices. Do not
            // publish those indices speculatively: flush the suffix at terminal.
            if matches!(
                event["item"]["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            ) || kind.starts_with("response.function_call_arguments.")
                || kind.starts_with("response.custom_tool_call_input.")
            {
                self.blocked = true;
            }
            if !self.blocked {
                if let Some(index) = event["output_index"].as_u64() {
                    let index = usize::try_from(index)
                        .map_err(|_| AdapterError::upstream_stream_invalid())?;
                    if index >= MAX_TOOLS {
                        return Err(AdapterError::upstream_stream_invalid());
                    }
                    self.seen.insert(index);
                    self.emit(event);
                }
            }
        }
        Ok(())
    }

    fn update_progress(&mut self) {
        let mut progress = crate::poison::mutex(&self.progress);
        progress.tool_use = self.tool_use.clone();
    }

    fn reject(&mut self, error: DiscoveryFailure) {
        self.update_progress();
        self.output.push_back(match error {
            DiscoveryFailure::Adapter(error) => failure(error.code(), error.parameter()),
            DiscoveryFailure::Upstream(error) => failure(error.category, None),
        });
        self.terminal = true;
    }

    async fn finish_round(&mut self) -> Result<(), DiscoveryFailure> {
        let mut event = self.event.clone();
        event.attempt = u16::from(self.discovery.budget.dispatches());
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        if !self.sse {
            if let Some(capture) = &self.settlement.upstream_usage {
                crate::poison::mutex(capture).finish_json_generation(&self.raw);
            }
        }
        let expected = self
            .settlement
            .runtime
            .block_degraded_routes_enabled()
            .then_some(self.settlement.route.source_model.as_str());
        if !self.status.is_success() {
            return Err(DiscoveryFailure::Upstream(
                AttemptFailure::status_with_body(self.status, Some(&self.raw))
                    .with_retry_headers(&self.headers),
            ));
        }
        let bytes = completed_upstream_response(&self.raw, true, expected)
            .map_err(|failure| DiscoveryFailure::Upstream(failure.failure))?;
        let mut terminal: Value = serde_json::from_slice(&bytes)
            .map_err(|_| AdapterError::upstream_response_invalid())?;
        populate_tokens(&mut event, &self.raw);
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
            if let Some(id) = &self.response_id {
                terminal["id"] = json!(id);
            }
            self.update_progress();
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
        if !self.discovery.budget.can_dispatch() {
            return Err(AdapterError::upstream_response_invalid()
                .with_parameter("output.relay.catalog.budget")
                .into());
        }
        record_lookup(
            &self.settlement.runtime,
            &self.discovery.session,
            &mut self.tool_use,
            &mut event,
        );
        self.discovery.budget.retain_input_bytes(
            crate::gateway::request_body::retained_request_bytes(&self.discovery.upstream),
        );
        self.tool_use.upstream_context = Some(crate::usage::RequestContextSize::capture(
            &self.discovery.upstream,
        ));
        self.update_progress();
        if let Some(capture) = &self.settlement.upstream_usage {
            crate::poison::mutex(capture).reset_generation();
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
            crate::transport::basis_points_progress_timeout(),
        );
        self.raw.clear();
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
        raw: Vec::new(),
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

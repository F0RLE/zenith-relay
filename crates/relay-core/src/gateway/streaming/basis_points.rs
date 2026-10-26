//! BPS wire normalization before the existing client protocol bridge.
//! Leading text is incremental; tools and their tail wait for atomic validation.

use super::UpstreamStream;
use crate::gateway::execution::basis_points::response::stream::emit_unseen_output;
use crate::gateway::execution::basis_points::response::translate_response_value;
use crate::protocol::completion::ResponseCompletion;
use crate::protocol::sse_data as sse_event_data;
use axum::body::Bytes;
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::time::Duration;

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOOLS: usize = 1024;

pub(super) mod discovery;
mod tools;
use tools::NativeTools;

fn failure_event(code: &str, parameter: Option<&str>) -> Value {
    let message = match parameter {
        Some("output.run_officejs.args.frame") => {
            crate::AdapterError::upstream_tool_arguments_frame_invalid().message()
        }
        Some("output.run_officejs.args.schema") => {
            crate::AdapterError::upstream_tool_arguments_invalid().message()
        }
        Some("output.run_officejs.args.json.eof") => {
            crate::AdapterError::upstream_tool_arguments_json_incomplete().message()
        }
        Some("output.run_officejs.args.json.syntax") => {
            crate::AdapterError::upstream_tool_arguments_json_syntax_invalid().message()
        }
        Some("output.run_officejs.args.json" | "output.run_officejs.args.object") => {
            crate::AdapterError::upstream_tool_arguments_json_invalid().message()
        }
        _ => "Basis Points stream could not be completed safely",
    };
    let mut body = json!({"type":"error","code":code,"message":message,"param":parameter});
    if let Some(parameter) = parameter {
        // The bounded diagnostics store preserves the message, not arbitrary
        // provider fields. This parameter is a local constant, never payload.
        body["message"] = json!(format!("{message} (parameter: {parameter})"));
    }
    body
}

fn failure(code: &str, parameter: Option<&str>) -> Bytes {
    failure_frame(failure_event(code, parameter))
}

fn adapter_failure_event(error: crate::AdapterError) -> Value {
    let mut body = failure_event(error.code(), error.parameter());
    body["message"] = json!(error
        .parameter()
        .map(|parameter| format!("{error} (parameter: {parameter})"))
        .unwrap_or_else(|| error.to_string()));
    body
}

fn adapter_failure(error: crate::AdapterError) -> Bytes {
    failure_frame(adapter_failure_event(error))
}

fn failure_frame(mut body: Value) -> Bytes {
    body["sequence_number"] = json!(0);
    Bytes::from(format!("event: error\ndata: {body}\n\n"))
}

pub(super) fn guard_progress(inner: UpstreamStream, timeout: Option<Duration>) -> UpstreamStream {
    Box::pin(stream::unfold(
        (
            inner,
            crate::transport::SseProgress::default(),
            timeout,
            false,
        ),
        move |(mut inner, mut progress, mut remaining, done)| async move {
            if done {
                return None;
            }
            let started = tokio::time::Instant::now();
            let next = match remaining {
                Some(deadline) => tokio::time::timeout(deadline, inner.next()).await,
                None => Ok(inner.next().await),
            };
            match next {
                Err(_) => Some((
                    Ok(failure(crate::error_codes::STREAM_IDLE_TIMEOUT, None)),
                    (inner, progress, remaining, true),
                )),
                Ok(Some(chunk)) => {
                    if let Ok(bytes) = &chunk {
                        remaining = if progress.observe(bytes) {
                            timeout
                        } else {
                            remaining.map(|left| left.saturating_sub(started.elapsed()))
                        };
                    }
                    Some((chunk, (inner, progress, remaining, false)))
                }
                Ok(None) => None,
            }
        },
    ))
}

struct Translator {
    request: Value,
    pending: Vec<u8>,
    output: VecDeque<Bytes>,
    tools: NativeTools,
    tool_barrier: Option<usize>,
    seen: HashSet<usize>,
    sequence: u64,
    terminal: bool,
}

impl Translator {
    fn new(request: Value) -> Self {
        Self {
            request,
            pending: Vec::new(),
            output: VecDeque::new(),
            tools: NativeTools::default(),
            tool_barrier: None,
            seen: HashSet::new(),
            sequence: 0,
            terminal: false,
        }
    }

    fn emit(&mut self, mut event: Value) {
        event["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        self.output.push_back(Bytes::from(format!(
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap_or("error")
        )));
    }

    fn reject(&mut self, parameter: &'static str) {
        self.fail(
            crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
            Some(parameter),
        );
    }

    fn fail(&mut self, code: &str, parameter: Option<&str>) {
        self.emit(failure_event(code, parameter));
        self.terminal = true;
        self.pending.clear();
    }

    fn event(&mut self, mut event: Value) -> Result<(), &'static str> {
        if event.get("type").is_none() && event["error"].is_object() {
            // The raw served-model guard uses the common untyped SSE error
            // envelope. Keep its category instead of relabeling it as an
            // adapter failure when normalizing the BPS stream.
            event["type"] = json!("error");
        }
        if event["type"] == "response.done" {
            // The shared Responses reader accepts this terminal alias. Its
            // body, not the alias alone, must still prove the terminal state.
            event["type"] = match ResponseCompletion::from_responses_event(&event) {
                Some(ResponseCompletion::Completed) => json!("response.completed"),
                Some(ResponseCompletion::Incomplete) => json!("response.incomplete"),
                _ => return Err("response.status"),
            };
        }
        let kind = event["type"].as_str().ok_or("stream.event.type")?;
        if ResponseCompletion::from_responses_event(&event) == Some(ResponseCompletion::Invalid) {
            return Err("response.status");
        }
        if let Some(index) = self.tools.observe(&event)? {
            // Expansion is unknown until terminal validation; later indexes may shift.
            self.tool_barrier = Some(self.tool_barrier.map_or(index, |before| before.min(index)));
            return Ok(());
        }
        if let Some(index) = event["output_index"].as_u64() {
            let index = usize::try_from(index).map_err(|_| "stream.output_index")?;
            if index >= MAX_TOOLS {
                return Err("stream.output_index");
            }
            if self.tool_barrier.is_some_and(|barrier| index >= barrier) {
                return Ok(());
            }
            // Search progress does not publish the output item itself.
            if !matches!(
                kind,
                "response.web_search_call.in_progress"
                    | "response.web_search_call.searching"
                    | "response.web_search_call.completed"
            ) {
                self.seen.insert(index);
            }
        }
        let terminal = matches!(
            kind,
            "response.incomplete"
                | "response.failed"
                | "response.cancelled"
                | "response.canceled"
                | "error"
        );
        if kind == "response.completed" {
            let response = event
                .get("response")
                .filter(|response| response["status"] == "completed")
                .ok_or("response.status")?;
            let items = response["output"].as_array().ok_or("response.output")?;
            self.tools.complete(items)?;
            // Terminal-only or out-of-order tools must not renumber emitted text.
            let seen_items: Vec<_> = self
                .seen
                .iter()
                .map(|&index| (index, items.get(index).cloned()))
                .collect();
            let response = event["response"].take();
            let translated = match translate_response_value(response, &self.request) {
                Ok(translated) => translated,
                Err(error) => {
                    self.emit(adapter_failure_event(error));
                    self.terminal = true;
                    self.pending.clear();
                    return Ok(());
                }
            };
            for (index, native) in seen_items {
                if native.as_ref()
                    != translated["output"]
                        .as_array()
                        .and_then(|items| items.get(index))
                    || native.is_none()
                {
                    return Err("stream.output_index");
                }
            }
            let seen = std::mem::take(&mut self.seen);
            emit_unseen_output(&translated, &seen, |event| self.emit(event));
            event["response"] = translated;
            self.terminal = true;
        } else if let Some(response) = event.get_mut("response").and_then(Value::as_object_mut) {
            // No native tool envelope may escape through lifecycle snapshots,
            // incomplete responses, failures, or cancellation.
            if let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) {
                output.retain(|item| {
                    !matches!(
                        item["type"].as_str(),
                        Some("function_call" | "custom_tool_call")
                    )
                });
            }
            response.insert(
                "metadata".into(),
                self.request
                    .get("metadata")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            );
        }
        self.terminal |= terminal;
        self.emit(event);
        Ok(())
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.terminal {
            return;
        }
        let mut pending = std::mem::take(&mut self.pending);
        let result = crate::protocol::sse::consume_bounded_frames(
            &mut pending,
            bytes,
            MAX_FRAME_BYTES,
            self,
            |state| state.terminal,
            Self::frame,
        );
        self.pending = pending;
        if result.is_err() {
            self.reject("stream.frame_size");
        }
    }

    fn frame(&mut self, frame: &[u8]) {
        let data = sse_event_data(frame);
        if data.is_empty() {
            return;
        }
        if data.trim_ascii() == b"[DONE]" {
            self.reject("stream.terminal");
            return;
        }
        match serde_json::from_slice::<Value>(&data) {
            Ok(mut event) if event.is_object() => {
                if event.get("type").is_none() {
                    // Match the shared reader's named terminal envelopes.
                    // Never infer success from an unrelated SSE event.
                    if let Some(kind) = crate::protocol::sse_lines(frame)
                        .find_map(|line| line.strip_prefix(b"event:"))
                        .and_then(|kind| std::str::from_utf8(kind.trim_ascii()).ok())
                        .filter(|kind| {
                            matches!(
                                *kind,
                                "response.completed"
                                    | "response.done"
                                    | "response.incomplete"
                                    | "response.failed"
                                    | "response.cancelled"
                                    | "response.canceled"
                                    | "error"
                            )
                        })
                    {
                        event["type"] = json!(kind);
                    }
                }
                if let Err(parameter) = self.event(event) {
                    self.reject(parameter);
                }
            }
            _ => self.reject("stream.event.json"),
        }
    }
}

pub(super) fn translate_stream(
    first: Bytes,
    remaining: UpstreamStream,
    request: Value,
) -> UpstreamStream {
    let mut translator = Translator::new(request);
    translator.push(&first);
    Box::pin(stream::unfold(
        (remaining, translator),
        |(mut inner, mut translator)| async move {
            loop {
                if let Some(bytes) = translator.output.pop_front() {
                    return Some((Ok(bytes), (inner, translator)));
                }
                if translator.terminal {
                    return None;
                }
                match inner.next().await {
                    Some(Ok(bytes)) => translator.push(&bytes),
                    Some(Err(error)) => {
                        translator.terminal = true;
                        return Some((Err(error), (inner, translator)));
                    }
                    None => translator.reject("stream.terminal"),
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests;

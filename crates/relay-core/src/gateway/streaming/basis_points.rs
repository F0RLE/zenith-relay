//! BPS wire normalization before the existing client protocol bridge.
//! Text is incremental; executable tools are released atomically at completion.

use super::UpstreamStream;
use crate::gateway::execution::basis_points::response::stream::emit_unseen_output;
use crate::gateway::execution::basis_points::response::translate_response_value;
use crate::protocol::completion::ResponseCompletion;
use crate::protocol::{sse_data as sse_event_data, take_sse_event};
use axum::body::Bytes;
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOOLS: usize = 1024;

fn failure(code: &str, parameter: Option<&str>) -> Bytes {
    let message = if parameter == Some("output.run_officejs.args.schema") {
        crate::AdapterError::upstream_tool_arguments_invalid().message()
    } else {
        "Basis Points stream could not be completed safely"
    };
    let mut body = json!({"type":"error","error":{"code":code,"message":message}});
    if let Some(parameter) = parameter {
        body["error"]["param"] = json!(parameter);
        // The bounded diagnostics store preserves the message, not arbitrary
        // provider fields. This parameter is a local constant, never payload.
        body["error"]["message"] = json!(format!("{message} (parameter: {parameter})"));
    }
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
    tools: HashMap<String, Option<String>>,
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
            tools: HashMap::new(),
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
        self.output.push_back(failure(
            crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
            Some(parameter),
        ));
        self.terminal = true;
        self.pending.clear();
    }

    fn remember_tool(&mut self, id: &str, call_id: Option<&str>) -> Result<(), ()> {
        if id.is_empty() || (!self.tools.contains_key(id) && self.tools.len() >= MAX_TOOLS) {
            return Err(());
        }
        let recorded = self.tools.entry(id.to_owned()).or_default();
        if let Some(call_id) = call_id {
            if call_id.is_empty() || recorded.as_deref().is_some_and(|before| before != call_id) {
                return Err(());
            }
            *recorded = Some(call_id.to_owned());
        }
        Ok(())
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
        let kind = event["type"]
            .as_str()
            .ok_or("stream.event.type")?
            .to_owned();
        if ResponseCompletion::from_responses_event(&event) == Some(ResponseCompletion::Invalid) {
            return Err("response.status");
        }
        let tool = matches!(
            event["item"]["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        );
        if kind.starts_with("response.function_call_arguments.")
            || kind.starts_with("response.custom_tool_call_input.")
        {
            self.remember_tool(
                event["item_id"].as_str().ok_or("stream.tool.item_id")?,
                None,
            )
            .map_err(|_| "stream.tool.identity")?;
            return Ok(());
        }
        if tool {
            if !matches!(
                kind.as_str(),
                "response.output_item.added" | "response.output_item.done"
            ) {
                return Err("stream.tool.event");
            }
            let call_id = event["item"]["call_id"].as_str();
            if kind == "response.output_item.done" && call_id.is_none() {
                return Err("stream.tool.call_id");
            }
            self.remember_tool(
                event["item"]["id"].as_str().ok_or("stream.tool.item_id")?,
                call_id,
            )
            .map_err(|_| "stream.tool.identity")?;
            return Ok(());
        }
        if let Some(index) = event["output_index"].as_u64() {
            let index = usize::try_from(index).map_err(|_| "stream.output_index")?;
            if index >= MAX_TOOLS {
                return Err("stream.output_index");
            }
            self.seen.insert(index);
        }
        if kind == "response.completed" {
            let response = event
                .get("response")
                .filter(|response| response["status"] == "completed")
                .ok_or("response.status")?;
            let items = response["output"].as_array().ok_or("response.output")?;
            for item in items.iter().filter(|item| {
                matches!(
                    item["type"].as_str(),
                    Some("function_call" | "custom_tool_call")
                )
            }) {
                if let Some(id) = item["id"].as_str() {
                    if let Some(recorded) = self.tools.remove(id).flatten() {
                        if item["call_id"].as_str() != Some(recorded.as_str()) {
                            return Err("stream.tool.call_id");
                        }
                    }
                }
            }
            if !self.tools.is_empty() || items.len() > MAX_TOOLS {
                return Err("stream.tool.output");
            }
            let response = event["response"].take();
            let translated = translate_response_value(response, &self.request)
                .map_err(|error| error.parameter().unwrap_or("response"))?;
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
        self.terminal |= matches!(
            kind.as_str(),
            "response.incomplete"
                | "response.failed"
                | "response.cancelled"
                | "response.canceled"
                | "error"
        );
        self.emit(event);
        Ok(())
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.terminal {
            return;
        }
        self.pending.extend_from_slice(bytes);
        while let Some(frame) = take_sse_event(&mut self.pending) {
            if frame.len() > MAX_FRAME_BYTES {
                self.reject("stream.frame_size");
                return;
            }
            let data = sse_event_data(&frame);
            if data.is_empty() {
                continue;
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
                        if let Some(kind) = crate::protocol::sse_lines(&frame)
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
            if self.terminal {
                self.pending.clear();
                return;
            }
        }
        if self.pending.len() > MAX_FRAME_BYTES {
            self.reject("stream.frame_size");
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

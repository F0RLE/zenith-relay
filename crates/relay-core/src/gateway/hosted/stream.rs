use super::{HostedExecutionError, HostedUnknown, Reply, ResponseCompletion, response_usage};
use crate::protocol::sse;
use axum::{body::Bytes, http::header::CONTENT_TYPE};
use serde_json::Value;
use std::{
    mem,
    time::{Duration, Instant},
};

mod generation;
use generation::Generation;
pub use generation::HostedGeneration;

#[cfg(test)]
mod tests;

const MAX_EVENT: usize = 1024 * 1024;
const MAX_STREAM: usize = 4 * 1024 * 1024;
const INVALID_RESPONSE: HostedExecutionError =
    HostedExecutionError::OutcomeUnknown(HostedUnknown::InvalidResponse);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedTerminalKind {
    Completed,
    Incomplete,
    Failed,
}

impl HostedTerminalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedStreamObservation {
    None,
    Usage([Option<i64>; 3]),
    Terminal {
        kind: HostedTerminalKind,
        usage: [Option<i64>; 3],
        text_delta: bool,
        generation: Option<HostedGeneration>,
    },
}

pub struct HostedStreamFrame {
    pub bytes: Bytes,
    pub observation: HostedStreamObservation,
}

/// Holds the physical core slot until the stream ends or is dropped.
pub struct HostedTextStream {
    reply: Option<Reply>,
    framing: Frames,
    event: Events,
    previous_usage: [Option<i64>; 3],
}

impl HostedTextStream {
    pub(super) fn new(reply: Reply, model: String) -> Result<Self, HostedExecutionError> {
        let mut types = reply.response.headers().get_all(CONTENT_TYPE).iter();
        let sse = types
            .next()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"));
        if reply.response.status() != reqwest::StatusCode::OK || !sse || types.next().is_some() {
            return Err(INVALID_RESPONSE);
        }
        Ok(Self {
            reply: Some(reply),
            framing: Frames::default(),
            event: Events {
                model,
                id: None,
                text_delta: false,
                sequence: 0,
                generation: Generation::default(),
            },
            previous_usage: [None; 3],
        })
    }

    pub fn next_sequence_number(&self) -> u64 {
        self.event.sequence
    }

    /// Do not measure upstream speed across a relay-imposed pause between text chunks.
    pub fn mark_read_pause(&mut self) {
        if let Some(at) = self.framing.chunk_at {
            self.event.generation.mark_read_pause(at);
        }
    }

    /// A missing terminal is an error, including EOF after complete text deltas.
    pub async fn next_frame(&mut self) -> Result<Option<HostedStreamFrame>, HostedExecutionError> {
        let Some(reply) = &mut self.reply else {
            return Ok(None);
        };
        let parsed = async {
            let (bytes, received_at) = self.framing.read(&mut reply.response).await?;
            let mut observation = self.event.observe(&bytes, received_at)?;
            if let HostedStreamObservation::Usage(usage) = observation {
                if usage == self.previous_usage {
                    observation = HostedStreamObservation::None;
                } else {
                    self.previous_usage = usage;
                }
            }
            Ok(HostedStreamFrame { bytes, observation })
        }
        .await;
        match &parsed {
            Ok(frame) => {
                if let HostedStreamObservation::Terminal { kind, .. } = frame.observation {
                    if kind == HostedTerminalKind::Completed {
                        reply
                            .lease
                            .settle_rotation_success_with_latency(crate::gateway::now_ms(), None);
                    }
                    self.reply.take();
                }
            }
            Err(_) => {
                self.reply.take();
            }
        }
        parsed.map(Some)
    }
}

#[derive(Default)]
struct Frames {
    pending: Vec<u8>,
    chunk: Bytes,
    chunk_at: Option<Instant>,
    used: usize,
}

impl Frames {
    async fn read(
        &mut self,
        response: &mut reqwest::Response,
    ) -> Result<(Bytes, Instant), HostedExecutionError> {
        use HostedExecutionError::OutcomeUnknown;
        loop {
            if self.chunk.is_empty() {
                self.chunk = tokio::time::timeout(Duration::from_secs(10), response.chunk())
                    .await
                    .map_err(|_| OutcomeUnknown(HostedUnknown::ReadTimeout))?
                    .map_err(|error| OutcomeUnknown(HostedUnknown::read_error(error)))?
                    .ok_or(OutcomeUnknown(HostedUnknown::MissingTerminal))?;
                self.chunk_at = Some(Instant::now());
            }
            if let Some(frame) = self.take()? {
                // Coalesced events share one timestamp, even if delivery pauses between them.
                return Ok((
                    frame,
                    self.chunk_at
                        .expect("a complete frame read needs a body chunk"),
                ));
            }
        }
    }

    fn take(&mut self) -> Result<Option<Bytes>, HostedExecutionError> {
        let limit = MAX_EVENT.min(MAX_STREAM - self.used);
        let prefix = &self.chunk[..self.chunk.len().min(limit - self.pending.len())];
        if self.pending.is_empty() {
            if let Some(end) = sse::event_end(prefix) {
                self.used += end;
                return Ok(Some(self.chunk.split_to(end)));
            }
            self.pending.extend_from_slice(prefix);
        } else {
            let old = self.pending.len();
            self.pending.extend_from_slice(prefix);
            if let Some(end) = sse::event_end_from(&self.pending, old - 1) {
                drop(self.chunk.split_to(end - old));
                self.pending.truncate(end);
                self.used += end;
                return Ok(Some(Bytes::from(mem::take(&mut self.pending))));
            }
        }
        if self.pending.len() == limit {
            return Err(HostedExecutionError::OutcomeUnknown(
                HostedUnknown::ResponseLimit,
            ));
        }
        self.chunk = Bytes::new();
        Ok(None)
    }
}

struct Events {
    model: String,
    id: Option<String>,
    text_delta: bool,
    sequence: u64,
    generation: Generation,
}

impl Events {
    fn observe(
        &mut self,
        frame: &[u8],
        at: Instant,
    ) -> Result<HostedStreamObservation, HostedExecutionError> {
        use HostedStreamObservation::None as NoObservation;
        std::str::from_utf8(frame).map_err(|_| INVALID_RESPONSE)?;
        let data = sse::event_data(frame);
        if data.is_empty() {
            return Ok(NoObservation);
        }
        let value =
            crate::strict_json::parse(std::str::from_utf8(&data).map_err(|_| INVALID_RESPONSE)?)
                .map_err(|_| INVALID_RESPONSE)?;
        let kind = value["type"].as_str().ok_or(INVALID_RESPONSE)?;
        if sse::lines(frame)
            .filter_map(|line| {
                line.strip_prefix(b"event:")
                    .or_else(|| (line == b"event").then_some(b""))
            })
            .map(|label| label.strip_prefix(b" ").unwrap_or(label))
            .last()
            .is_some_and(|label| !label.is_empty() && label != kind.as_bytes())
        {
            return Err(INVALID_RESPONSE);
        }
        let observation = self.observe_value(kind, &value, at)?;
        self.sequence = self
            .sequence
            .max(value["sequence_number"].as_u64().unwrap_or(self.sequence))
            .saturating_add(1);
        Ok(observation)
    }

    fn observe_value(
        &mut self,
        kind: &str,
        value: &Value,
        at: Instant,
    ) -> Result<HostedStreamObservation, HostedExecutionError> {
        use HostedStreamObservation::{None as NoObservation, Terminal};
        let response = &value["response"];
        let id = response["id"].as_str();
        let model = response["model"].as_str();
        let status = response["status"].as_str();
        if kind != "response.failed" && response.get("error").is_some_and(|v| !v.is_null()) {
            return Err(INVALID_RESPONSE);
        }
        if kind == "response.created" {
            let id = id
                .filter(|id| crate::is_ascii_token(id, 256))
                .ok_or(INVALID_RESPONSE)?;
            if self.id.is_some()
                || model != Some(&self.model)
                || !matches!(status, Some("queued" | "in_progress"))
            {
                return Err(INVALID_RESPONSE);
            }
            self.id = Some(id.into());
            return Ok(progress(response));
        }
        if self.id.is_none()
            || id.is_some_and(|id| Some(id) != self.id.as_deref())
            || model.is_some_and(|model| model != self.model)
            || value
                .get("response_id")
                .is_some_and(|v| v.as_str() != self.id.as_deref())
        {
            return Err(INVALID_RESPONSE);
        }
        let terminal = match kind {
            "response.completed" | "response.incomplete" | "response.failed" => {
                if id != self.id.as_deref() || model != Some(&self.model) {
                    return Err(INVALID_RESPONSE);
                }
                if status != kind.strip_prefix("response.") || !response["output"].is_array() {
                    return Err(INVALID_RESPONSE);
                }
                match (
                    kind,
                    ResponseCompletion::for_protocol(crate::WireApi::Responses, response),
                ) {
                    ("response.completed", ResponseCompletion::Completed) => {
                        HostedTerminalKind::Completed
                    }
                    ("response.incomplete", ResponseCompletion::Incomplete) => {
                        HostedTerminalKind::Incomplete
                    }
                    ("response.failed", _) => HostedTerminalKind::Failed,
                    _ => return Err(INVALID_RESPONSE),
                }
            }
            "response.in_progress" | "response.queued" => {
                if id != self.id.as_deref()
                    || model != Some(&self.model)
                    || status != kind.strip_prefix("response.")
                {
                    return Err(INVALID_RESPONSE);
                }
                return Ok(progress(response));
            }
            "response.output_text.delta" => {
                let delta = value["delta"].as_str().ok_or(INVALID_RESPONSE)?;
                self.text_delta |= !delta.trim().is_empty();
                self.generation.delta(value, delta, at);
                return Ok(NoObservation);
            }
            "response.done" | "error" => return Err(INVALID_RESPONSE),
            _ => return Ok(NoObservation),
        };
        let usage = response_usage(response);
        Ok(Terminal {
            kind: terminal,
            usage,
            text_delta: self.text_delta,
            generation: if terminal == HostedTerminalKind::Completed && self.text_delta {
                self.generation.complete(response, usage[2])
            } else {
                None
            },
        })
    }
}

fn progress(response: &Value) -> HostedStreamObservation {
    let usage = response_usage(response);
    if usage == [None; 3] {
        HostedStreamObservation::None
    } else {
        HostedStreamObservation::Usage(usage)
    }
}

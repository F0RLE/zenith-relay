use super::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod chat;
mod gemini;
mod messages;
mod output;
mod responses;

#[derive(Debug)]
pub struct TranslationStream {
    translation_request: TranslationRequest,
    decoded_response: Response,
    pending: Vec<u8>,
    pending_frames: VecDeque<Vec<u8>>,
    indices: BTreeMap<String, usize>,
    response_annotations: BTreeMap<(u64, u64, u64), citations::UrlCitation>,
    response_items: BTreeMap<u64, responses::OutputItemState>,
    chat_completion: crate::protocol::completion::ChatStreamCompletion,
    chat_annotations: Vec<Value>,
    usage_snapshot: Option<Value>,
    messages_completion: crate::protocol::completion::MessagesStreamCompletion,
    gemini_completion: crate::protocol::completion::GeminiStreamCompletion,
    native_indices: BTreeMap<String, usize>,
    native_closed: BTreeSet<usize>,
    gemini_calls: super::super::gemini::GeminiFunctionCalls,
    closed: BTreeSet<usize>,
    emitted: Vec<Block>,
    finished_message_blocks: usize,
    started: bool,
    finish_reason: Option<Finish>,
    saw_refusal: bool,
    refusal_explanation: Option<String>,
    terminal: bool,
    completed: Option<TranslatedResponse>,
    upstream_error: Option<Value>,
    sequence: u64,
}

impl TranslationStream {
    pub fn new(translation_request: TranslationRequest) -> Self {
        let decoded_response = Response {
            id: translation_request.response_id.clone(),
            blocks: Vec::new(),
            citations: BTreeMap::new(),
            usage: Usage::default(),
            finish: Finish::Stop,
            native_output: Vec::new(),
        };
        Self {
            translation_request,
            decoded_response,
            pending: Vec::new(),
            pending_frames: VecDeque::new(),
            indices: BTreeMap::new(),
            response_annotations: BTreeMap::new(),
            response_items: BTreeMap::new(),
            chat_completion: crate::protocol::completion::ChatStreamCompletion::default(),
            chat_annotations: Vec::new(),
            usage_snapshot: None,
            messages_completion: crate::protocol::completion::MessagesStreamCompletion::default(),
            gemini_completion: crate::protocol::completion::GeminiStreamCompletion::default(),
            native_indices: BTreeMap::new(),
            native_closed: BTreeSet::new(),
            gemini_calls: super::super::gemini::GeminiFunctionCalls::default(),
            closed: BTreeSet::new(),
            emitted: Vec::new(),
            finished_message_blocks: 0,
            started: false,
            finish_reason: None,
            saw_refusal: false,
            refusal_explanation: None,
            terminal: false,
            completed: None,
            upstream_error: None,
            sequence: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.terminal {
            return;
        }
        let pending = crate::protocol::push_pending_sse_frames(
            std::mem::take(&mut self.pending),
            bytes,
            self,
            |stream| stream.terminal,
            |stream, event| {
                if let Err(error) = stream.consume(event) {
                    stream.fail(error);
                }
            },
        );
        match pending {
            Ok(pending) => self.pending = pending,
            Err(()) => self
                .fail(AdapterError::upstream_stream_invalid().with_parameter("stream.event_size")),
        }
    }

    pub fn finish(&mut self) {
        if self.terminal {
            return;
        }
        if self.awaiting_upstream_end() && self.pending.iter().all(u8::is_ascii_whitespace) {
            if let Err(error) = self.complete() {
                self.fail(error);
            }
        } else {
            self.fail(AdapterError::upstream_stream_invalid().with_parameter("stream.terminal"));
        }
    }
    pub(in crate::protocol::adapter) fn awaiting_upstream_end(&self) -> bool {
        !self.terminal
            && self.translation_request.upstream == WireApi::Gemini
            && self.finish_reason.is_some()
    }
    pub(in crate::protocol::adapter) fn abort(&mut self) {
        if !self.terminal {
            self.fail(AdapterError::upstream_stream_invalid().with_parameter("stream.transport"));
        }
    }
    pub fn pop_output(&mut self) -> Option<Vec<u8>> {
        self.pending_frames.pop_front()
    }
    pub fn completed(&self) -> Option<&TranslatedResponse> {
        self.completed.as_ref()
    }
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }
    pub fn take_upstream_error(&mut self) -> Option<Value> {
        self.upstream_error.take()
    }

    fn consume(&mut self, bytes: &[u8]) -> AdapterResult<()> {
        std::str::from_utf8(bytes).map_err(|_| {
            AdapterError::upstream_stream_invalid().with_parameter("stream.encoding")
        })?;
        let sse_payload = crate::protocol::sse_data(bytes);
        if sse_payload.is_empty() {
            return Ok(());
        }
        if sse_payload.as_ref() == b"[DONE]" {
            let completed = match self.translation_request.upstream {
                WireApi::ChatCompletions => self.chat_completion.outcome().is_terminal(),
                WireApi::Gemini => self.awaiting_upstream_end(),
                _ => false,
            };
            if !completed || self.finish_reason.is_none() {
                return Err(AdapterError::upstream_stream_invalid().with_parameter("stream.done"));
            }
            return self.complete();
        }
        let event_payload: Value = serde_json::from_slice(&sse_payload)
            .map_err(|_| AdapterError::upstream_stream_invalid().with_parameter("stream.json"))?;
        self.observe_usage(&event_payload);
        if event_payload
            .get("error")
            .is_some_and(|error| !error.is_null())
            || event_payload.get("type").and_then(Value::as_str) == Some("error")
        {
            self.upstream_error = Some(event_payload);
            return Err(AdapterError::upstream_stream_invalid());
        }
        let terminal = match self.translation_request.upstream {
            WireApi::ChatCompletions => self.chat(&event_payload)?,
            WireApi::Responses => self.responses(&event_payload).map_err(|error| {
                if error.parameter().is_some() {
                    error
                } else {
                    error.with_parameter(responses::diagnostic_parameter(&event_payload))
                }
            })?,
            WireApi::Messages => self.messages(&event_payload)?,
            WireApi::Gemini => self.gemini(&event_payload)?,
        };
        if terminal && self.translation_request.upstream != WireApi::Gemini {
            self.complete().map_err(|error| {
                if error.parameter().is_some() {
                    error
                } else {
                    error.with_parameter("stream.complete")
                }
            })?;
        } else {
            self.emit_changes(false).map_err(|error| {
                if error.parameter().is_some() {
                    error
                } else {
                    error.with_parameter("stream.output")
                }
            })?;
        }
        Ok(())
    }

    fn observe_usage(&mut self, payload: &Value) {
        let field = if self.translation_request.upstream == WireApi::Gemini {
            "usageMetadata"
        } else {
            "usage"
        };
        let usage = payload
            .get(field)
            .or_else(|| payload.pointer("/message/usage"))
            .or_else(|| payload.pointer("/response/usage"));
        if let Some(usage) = usage {
            crate::protocol::merge_usage(&mut self.usage_snapshot, usage);
            // Recompute from the merged raw counters so overflow becomes unknown,
            // rather than retaining an older, lower derived value.
            let mut usage_wrapper = serde_json::Map::new();
            usage_wrapper.insert(
                field.to_owned(),
                self.usage_snapshot.clone().unwrap_or(Value::Null),
            );
            self.decoded_response.usage = response::usage(
                self.translation_request.upstream,
                &Value::Object(usage_wrapper),
            );
        }
    }

    fn insert(&mut self, key: String, block: Block) -> AdapterResult<usize> {
        if self.indices.contains_key(&key) {
            return Err(AdapterError::upstream_stream_invalid());
        }
        let index = self.decoded_response.blocks.len();
        self.indices.insert(key, index);
        self.decoded_response.blocks.push(block);
        Ok(index)
    }

    fn complete(&mut self) -> AdapterResult<()> {
        if self.translation_request.upstream == WireApi::ChatCompletions {
            citations::set_chat_annotations(
                &mut self.decoded_response,
                Some(&Value::Array(std::mem::take(&mut self.chat_annotations))),
            )?;
        }
        if self.translation_request.upstream == WireApi::Gemini
            && !self.gemini_completion.outcome().is_terminal()
        {
            return Err(AdapterError::upstream_stream_invalid());
        }
        self.decoded_response.finish = self
            .finish_reason
            .ok_or_else(AdapterError::upstream_stream_invalid)?;
        if self.translation_request.upstream == WireApi::Gemini
            && self.decoded_response.finish == Finish::Stop
            && self.decoded_response.blocks.is_empty()
        {
            return Err(AdapterError::upstream_stream_invalid());
        }
        response::validate_calls(&self.decoded_response.blocks)?;
        let completed_response = self
            .translation_request
            .complete(self.decoded_response.clone())?;
        self.emit_changes(true)?;
        self.emit_end(&completed_response.response_body)?;
        self.completed = Some(completed_response);
        self.terminal = true;
        Ok(())
    }

    fn fail(&mut self, failure: AdapterError) {
        self.terminal = true;
        if self.translation_request.client == WireApi::Responses
            && self
                .upstream_error
                .as_ref()
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str)
                == Some("response.failed")
        {
            if let Some(event) = self.upstream_error.clone() {
                self.event("response.failed", event);
                return;
            }
        }
        // Adapter errors contain only static diagnostics. Keep their cause and
        // field in the error envelope without echoing any provider payload.
        let message = match failure.parameter() {
            Some(parameter) => format!(
                "Upstream stream conversion failed at {parameter}: {}",
                failure
            ),
            None => format!("Upstream stream conversion failed: {failure}"),
        };
        let mut error = json!({"type":"api_error","code":crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,"message":message});
        if let Some(parameter) = failure.parameter() {
            error["param"] = parameter.into();
        }
        match self.translation_request.client {
            WireApi::Responses => self.event("response.failed", json!({"type":"response.failed","response":{"id":self.decoded_response.id,"object":"response","status":"failed","output":[],"error":error}})),
            WireApi::Messages => self.event("error", json!({"type":"error","error":error})),
            _ => self.event("", json!({"error":error})),
        }
    }
}

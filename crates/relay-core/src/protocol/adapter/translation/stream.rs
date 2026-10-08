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
    closed: BTreeSet<usize>,
    emitted: Vec<Block>,
    started: bool,
    finish_reason: Option<Finish>,
    saw_refusal: bool,
    terminal: bool,
    completed: Option<MessagesBridgeResponse>,
    upstream_error: Option<Value>,
    sequence: u64,
}

impl TranslationStream {
    pub fn new(translation_request: TranslationRequest) -> Self {
        let decoded_response = Response {
            id: translation_request.response_id.clone(),
            blocks: Vec::new(),
            usage: Usage::default(),
            finish: Finish::Stop,
        };
        Self {
            translation_request,
            decoded_response,
            pending: Vec::new(),
            pending_frames: VecDeque::new(),
            indices: BTreeMap::new(),
            closed: BTreeSet::new(),
            emitted: Vec::new(),
            started: false,
            finish_reason: None,
            saw_refusal: false,
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
        self.pending = crate::protocol::push_pending_sse_frames(
            std::mem::take(&mut self.pending),
            bytes,
            self,
            |stream| stream.terminal,
            |stream, event| {
                if stream.consume(event).is_err() {
                    stream.fail();
                }
            },
        );
    }

    pub fn finish(&mut self) {
        if !self.terminal {
            self.fail();
        }
    }
    pub fn pop_output(&mut self) -> Option<Vec<u8>> {
        self.pending_frames.pop_front()
    }
    pub fn completed(&self) -> Option<&MessagesBridgeResponse> {
        self.completed.as_ref()
    }
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }
    pub fn take_upstream_error(&mut self) -> Option<Value> {
        self.upstream_error.take()
    }

    fn consume(&mut self, bytes: &[u8]) -> AdapterResult<()> {
        std::str::from_utf8(bytes).map_err(|_| AdapterError::upstream_stream_invalid())?;
        let sse_payload = crate::protocol::sse_data(bytes);
        if sse_payload.is_empty() {
            return Ok(());
        }
        if sse_payload == b"[DONE]" {
            if self.translation_request.upstream != WireApi::ChatCompletions
                || self.finish_reason.is_none()
            {
                return Err(AdapterError::upstream_stream_invalid());
            }
            return self.complete();
        }
        let event_payload: Value = serde_json::from_slice(&sse_payload)
            .map_err(|_| AdapterError::upstream_stream_invalid())?;
        if event_payload
            .get("error")
            .is_some_and(|error| !error.is_null())
            || event_payload.get("type").and_then(Value::as_str) == Some("error")
        {
            self.upstream_error = Some(event_payload);
            return Err(AdapterError::upstream_stream_invalid());
        }
        self.merge_usage(response::usage(
            self.translation_request.upstream,
            &event_payload,
        ));
        let terminal = match self.translation_request.upstream {
            WireApi::ChatCompletions => self.chat(&event_payload)?,
            WireApi::Responses => self.responses(&event_payload)?,
            WireApi::Messages => self.messages(&event_payload)?,
            WireApi::Gemini => self.gemini(&event_payload)?,
        };
        self.emit_changes()?;
        if terminal {
            self.complete()?;
        }
        Ok(())
    }

    fn merge_usage(&mut self, usage: Usage) {
        let accumulated_usage = &mut self.decoded_response.usage;
        accumulated_usage.input = usage.input.or(accumulated_usage.input);
        accumulated_usage.output = usage.output.or(accumulated_usage.output);
        accumulated_usage.total = usage.total.or(accumulated_usage.total);
        accumulated_usage.cached = usage.cached.or(accumulated_usage.cached);
        accumulated_usage.reasoning = usage.reasoning.or(accumulated_usage.reasoning);
        accumulated_usage.cache_write = usage.cache_write.or(accumulated_usage.cache_write);
        accumulated_usage.cache_write_5m =
            usage.cache_write_5m.or(accumulated_usage.cache_write_5m);
        accumulated_usage.cache_write_1h =
            usage.cache_write_1h.or(accumulated_usage.cache_write_1h);
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
        self.decoded_response.finish = self
            .finish_reason
            .ok_or_else(AdapterError::upstream_stream_invalid)?;
        response::validate_calls(&self.decoded_response.blocks)?;
        let completed_response = self
            .translation_request
            .clone()
            .complete(self.decoded_response.clone())?;
        self.emit_changes()?;
        self.emit_end(&completed_response.response_body)?;
        self.completed = Some(completed_response);
        self.terminal = true;
        Ok(())
    }

    fn fail(&mut self) {
        self.terminal = true;
        let error = json!({"type":"api_error","code":crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,"message":"Upstream stream cannot be represented by the selected route"});
        match self.translation_request.client {
            WireApi::Responses => self.event("response.failed", json!({"type":"response.failed","response":{"id":self.decoded_response.id,"object":"response","status":"failed","output":[],"error":error}})),
            WireApi::Messages => self.event("error", json!({"type":"error","error":error})),
            _ => self.event("", json!({"error":error})),
        }
    }
}

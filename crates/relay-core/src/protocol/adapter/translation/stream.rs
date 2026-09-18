use super::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod input;
mod output;

const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct TranslationStream {
    request: TranslationRequest,
    response: Response,
    pending: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    indices: BTreeMap<String, usize>,
    closed: BTreeSet<usize>,
    emitted: Vec<Block>,
    started: bool,
    finish_reason: Option<Finish>,
    terminal: bool,
    completed: Option<MessagesBridgeResponse>,
    upstream_error: Option<Value>,
    received: usize,
    sequence: u64,
}

impl TranslationStream {
    pub fn new(request: TranslationRequest) -> Self {
        let response = Response {
            id: request.response_id.clone(),
            blocks: Vec::new(),
            usage: Usage::default(),
            finish: Finish::Stop,
        };
        Self {
            request,
            response,
            pending: Vec::new(),
            output: VecDeque::new(),
            indices: BTreeMap::new(),
            closed: BTreeSet::new(),
            emitted: Vec::new(),
            started: false,
            finish_reason: None,
            terminal: false,
            completed: None,
            upstream_error: None,
            received: 0,
            sequence: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.terminal {
            return;
        }
        self.received = self.received.saturating_add(bytes.len());
        if self.pending.len().saturating_add(bytes.len()) > MAX_EVENT_BYTES
            || self.received > MAX_TRANSCRIPT_BYTES
        {
            self.fail();
            return;
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = crate::protocol::sse_event_end(&self.pending) {
            let event = self.pending.drain(..end).collect::<Vec<_>>();
            if self.consume(&event).is_err() {
                self.fail();
            }
            if self.terminal {
                self.pending.clear();
                break;
            }
        }
    }

    pub fn finish(&mut self) {
        if !self.terminal {
            self.fail();
        }
    }
    pub fn pop_output(&mut self) -> Option<Vec<u8>> {
        self.output.pop_front()
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
        let text =
            std::str::from_utf8(bytes).map_err(|_| AdapterError::upstream_stream_invalid())?;
        let data = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .map(|value| value.strip_prefix(' ').unwrap_or(value))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            if self.request.upstream != WireApi::ChatCompletions || self.finish_reason.is_none() {
                return Err(AdapterError::upstream_stream_invalid());
            }
            return self.complete();
        }
        let value: Value =
            serde_json::from_str(&data).map_err(|_| AdapterError::upstream_stream_invalid())?;
        if value.get("error").is_some_and(|error| !error.is_null())
            || value.get("type").and_then(Value::as_str) == Some("error")
        {
            self.upstream_error = Some(value);
            return Err(AdapterError::upstream_stream_invalid());
        }
        self.merge_usage(response::usage(self.request.upstream, &value));
        let terminal = match self.request.upstream {
            WireApi::ChatCompletions => self.chat(&value)?,
            WireApi::Responses => self.responses(&value)?,
            WireApi::Messages => self.messages(&value)?,
            WireApi::Gemini => self.gemini(&value)?,
        };
        self.emit_changes()?;
        if terminal {
            self.complete()?;
        }
        Ok(())
    }

    fn merge_usage(&mut self, usage: Usage) {
        let current = &mut self.response.usage;
        current.input = usage.input.or(current.input);
        current.output = usage.output.or(current.output);
        current.total = usage.total.or(current.total);
        current.cached = usage.cached.or(current.cached);
        current.reasoning = usage.reasoning.or(current.reasoning);
        current.cache_write = usage.cache_write.or(current.cache_write);
        current.cache_write_5m = usage.cache_write_5m.or(current.cache_write_5m);
        current.cache_write_1h = usage.cache_write_1h.or(current.cache_write_1h);
    }

    fn insert(&mut self, key: String, block: Block) -> AdapterResult<usize> {
        if self.indices.contains_key(&key) {
            return Err(AdapterError::upstream_stream_invalid());
        }
        let index = self.response.blocks.len();
        self.indices.insert(key, index);
        self.response.blocks.push(block);
        Ok(index)
    }

    fn complete(&mut self) -> AdapterResult<()> {
        self.response.finish = self
            .finish_reason
            .ok_or_else(AdapterError::upstream_stream_invalid)?;
        response::validate_calls(&self.response.blocks)?;
        let completed = self.request.clone().complete(self.response.clone())?;
        self.emit_changes()?;
        self.emit_end(&completed.response_body)?;
        self.completed = Some(completed);
        self.terminal = true;
        Ok(())
    }

    fn fail(&mut self) {
        self.terminal = true;
        let error = json!({"type":"api_error","code":crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,"message":"Upstream stream cannot be represented by the selected route"});
        match self.request.client {
            WireApi::Responses => self.event("response.failed", json!({"type":"response.failed","response":{"id":self.response.id,"object":"response","status":"failed","output":[],"error":error}})),
            WireApi::Messages => self.event("error", json!({"type":"error","error":error})),
            _ => self.event("", json!({"error":error})),
        }
    }
}

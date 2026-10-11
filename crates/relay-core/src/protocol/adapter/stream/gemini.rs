use super::super::contracts::{AdapterError, MessagesBridgeResponse};
use super::super::gemini::{GeminiBridgeRequest, GeminiFunctionCalls};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

mod events;

/// Incrementally converts Gemini's native `streamGenerateContent` SSE frames
/// into the client-facing Responses event contract. Gemini can send text,
/// thought text, and function calls as separate parts; all are retained until
/// EOF so late usage and the native assistant turn survive conversion.
#[derive(Debug)]
pub struct GeminiStreamBridge {
    request: GeminiBridgeRequest,
    pending: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    native_parts: Vec<Value>,
    active_text: Option<(bool, usize)>,
    calls: BTreeMap<usize, GeminiStreamCall>,
    function_calls: GeminiFunctionCalls,
    completion: crate::protocol::completion::GeminiStreamCompletion,
    usage: Option<Value>,
    started: bool,
    next_output_index: usize,
    finished_upstream: bool,
    finish_reason: Option<String>,
    prompt_response: Option<Value>,
    completed: Option<MessagesBridgeResponse>,
    terminal: bool,
    sequence_number: u64,
    upstream_error: Option<Value>,
}

#[derive(Clone, Debug)]
struct GeminiStreamCall {
    id: String,
    native_part_index: usize,
}

impl GeminiStreamBridge {
    pub fn new(request: GeminiBridgeRequest) -> Self {
        Self {
            request,
            pending: Vec::new(),
            output: VecDeque::new(),
            native_parts: Vec::new(),
            active_text: None,
            calls: BTreeMap::new(),
            function_calls: GeminiFunctionCalls::default(),
            completion: crate::protocol::completion::GeminiStreamCompletion::default(),
            usage: None,
            started: false,
            next_output_index: 0,
            finished_upstream: false,
            finish_reason: None,
            prompt_response: None,
            completed: None,
            terminal: false,
            sequence_number: 0,
            upstream_error: None,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.pending = crate::protocol::push_pending_sse_frames(
            std::mem::take(&mut self.pending),
            bytes,
            self,
            |bridge| bridge.terminal,
            |bridge, event| bridge.handle_event(event),
        );
    }

    pub fn finish(&mut self) {
        if self.terminal {
            return;
        }
        if self.finished_upstream && self.pending.iter().all(u8::is_ascii_whitespace) {
            self.complete();
        } else {
            self.fail(AdapterError::upstream_stream_invalid());
        }
    }

    pub(in crate::protocol::adapter) fn awaiting_upstream_end(&self) -> bool {
        !self.terminal && self.finished_upstream
    }

    pub(in crate::protocol::adapter) fn abort(&mut self) {
        self.fail(AdapterError::upstream_stream_invalid());
    }

    pub fn pop_output(&mut self) -> Option<Vec<u8>> {
        self.output.pop_front()
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub fn completed(&self) -> Option<&MessagesBridgeResponse> {
        self.completed.as_ref()
    }

    pub fn take_upstream_error(&mut self) -> Option<Value> {
        self.upstream_error.take()
    }
}

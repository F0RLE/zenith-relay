use super::super::contracts::{AdapterError, MessagesBridgeResponse, ResponsesToolKind};
use super::super::gemini::GeminiBridgeRequest;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

mod events;

/// Incrementally converts Gemini's native `streamGenerateContent` SSE frames
/// into the client-facing Responses event contract. Gemini can send text,
/// thought text, and function calls as separate parts; all are retained until
/// the terminal chunk so the exact assistant turn can be used for continuation.
#[derive(Debug)]
pub struct GeminiStreamBridge {
    request: GeminiBridgeRequest,
    pending: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    text: String,
    thinking: String,
    calls: BTreeMap<usize, GeminiStreamCall>,
    active_call: Option<usize>,
    order: Vec<GeminiStreamOutput>,
    usage: Option<Value>,
    started: bool,
    text_started: bool,
    text_output_index: usize,
    next_output_index: usize,
    thinking_output_index: Option<usize>,
    finished_upstream: bool,
    finish_reason: Option<String>,
    completed: Option<MessagesBridgeResponse>,
    terminal: bool,
    upstream_error: Option<Value>,
}

#[derive(Clone, Debug)]
struct GeminiStreamCall {
    id: String,
    item_id: String,
    name: String,
    kind: ResponsesToolKind,
    args: Value,
    output_index: usize,
    thought_signature: Option<String>,
    emitted_arguments: String,
}

#[derive(Clone, Debug)]
enum GeminiStreamOutput {
    Text,
    Thinking,
    Call(usize),
}

impl GeminiStreamBridge {
    pub fn new(request: GeminiBridgeRequest) -> Self {
        Self {
            request,
            pending: Vec::new(),
            output: VecDeque::new(),
            text: String::new(),
            thinking: String::new(),
            calls: BTreeMap::new(),
            active_call: None,
            order: Vec::new(),
            usage: None,
            started: false,
            text_started: false,
            text_output_index: 0,
            next_output_index: 0,
            thinking_output_index: None,
            finished_upstream: false,
            finish_reason: None,
            completed: None,
            terminal: false,
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
        if self.finished_upstream {
            self.complete();
        } else {
            self.fail(AdapterError::upstream_stream_invalid());
        }
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

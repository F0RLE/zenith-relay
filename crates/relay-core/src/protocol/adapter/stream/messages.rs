use super::super::contracts::{
    AdapterError, MessagesBridgeRequest, MessagesBridgeResponse, ResponsesToolKind,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod events;

/// Incremental Messages-to-Responses state machine. It owns no network
/// client and can therefore be reused by desktop, server, and contract tests.
#[derive(Debug)]
pub struct MessagesStreamBridge {
    request: Option<MessagesBridgeRequest>,
    model: String,
    pending: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    assistant_blocks: BTreeMap<usize, StreamBlock>,
    closed_blocks: BTreeSet<usize>,
    response_id: Option<String>,
    upstream_id: Option<String>,
    usage: Option<Value>,
    stop_reason: Option<String>,
    text_output: Option<TextOutput>,
    next_output_index: usize,
    next_message_index: usize,
    completed: Option<MessagesBridgeResponse>,
    upstream_error: Option<Value>,
    terminal: bool,
}
#[derive(Clone, Debug)]
enum StreamBlock {
    Text {
        text: String,
        content_index: Option<usize>,
        output_index: Option<usize>,
    },
    Tool {
        id: String,
        item_id: String,
        upstream_name: String,
        name: String,
        namespace: Option<String>,
        kind: ResponsesToolKind,
        arguments: String,
        output_index: usize,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
}

#[derive(Debug)]
enum StreamDelta {
    Tool {
        item_id: String,
        output_index: usize,
        delta: String,
    },
    NoOutput,
}

#[derive(Debug)]
struct TextOutput {
    item_id: String,
    output_index: usize,
    next_content_index: usize,
}

impl MessagesStreamBridge {
    pub fn new(request: MessagesBridgeRequest) -> Self {
        Self {
            model: request.state.model.clone(),
            request: Some(request),
            pending: Vec::new(),
            output: VecDeque::new(),
            assistant_blocks: BTreeMap::new(),
            closed_blocks: BTreeSet::new(),
            response_id: None,
            upstream_id: None,
            usage: None,
            stop_reason: None,
            text_output: None,
            next_output_index: 0,
            next_message_index: 0,
            completed: None,
            upstream_error: None,
            terminal: false,
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
        self.fail(AdapterError::upstream_stream_invalid());
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
}

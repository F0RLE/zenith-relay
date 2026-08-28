use super::contracts::{
    custom_tool_item_id, AdapterError, AdapterResult, MessagesBridgeRequest,
    MessagesBridgeResponse, ResponsesToolKind,
};
use super::gemini::{apply_partial_args, function_call_args, GeminiBridgeRequest};
use super::messages::{
    bridged_response_id_scoped, custom_tool_input, responses_output_from_messages_content,
    responses_usage, set_message_output_id, validate_messages_tool_calls,
};
use crate::protocol::sse_event_end;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub enum AdapterStreamBridge {
    Messages(Box<MessagesStreamBridge>),
    Gemini(Box<GeminiStreamBridge>),
}

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
            text_output: None,
            next_output_index: 0,
            next_message_index: 0,
            completed: None,
            upstream_error: None,
            terminal: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.terminal {
            return;
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.pending) {
            let event = self.pending.drain(..end).collect::<Vec<_>>();
            self.handle_event(&event);
            if self.terminal {
                self.pending.clear();
                return;
            }
        }
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

    fn handle_event(&mut self, event: &[u8]) {
        let Some(value) = parse_sse_data(event) else {
            if sse_event_has_data(event) {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        match kind {
            "message_start" => self.handle_message_start(&value),
            "content_block_start" => self.handle_block_start(&value),
            "content_block_delta"
                if value
                    .get("delta")
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|delta| {
                        matches!(delta, "citations_delta" | "document" | "compaction_delta")
                    }) => {}
            "content_block_delta" => self.handle_block_delta(&value),
            "content_block_stop" => self.handle_block_stop(&value),
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    self.usage = Some(usage.clone());
                }
            }
            "message_stop" => self.complete(),
            // Anthropic may emit keep-alives and metadata deltas which do not
            // change the client-visible Responses output. They must not turn
            // an otherwise valid stream into a synthetic adapter failure.
            "ping" => {}
            "error" => {
                self.upstream_error = Some(value.clone());
                self.fail(AdapterError::upstream_stream_invalid());
            }
            kind if is_ignorable_metadata_event(kind) => {}
            _ => self.fail(AdapterError::upstream_stream_invalid()),
        }
    }

    fn handle_message_start(&mut self, value: &Value) {
        let Some(message) = value.get("message") else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(upstream_id) = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if self.response_id.is_some() {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let response_scope = self
            .request
            .as_ref()
            .map_or("", MessagesBridgeRequest::response_scope);
        let response_id = bridged_response_id_scoped(response_scope, upstream_id);
        self.upstream_id = Some(upstream_id.to_string());
        self.response_id = Some(response_id.clone());
        if let Some(usage) = message.get("usage") {
            self.usage = Some(usage.clone());
        }
        self.frame(
            "response.created",
            json!({
                "type": "response.created",
                "response": {
                    "id": response_id,
                    "object": "response",
                    "status": "in_progress",
                    "model": self.model,
                    "output": [],
                }
            }),
        );
    }

    fn handle_block_start(&mut self, value: &Value) {
        let Some(index) = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(block) = value.get("content_block").and_then(Value::as_object) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if self.response_id.is_none()
            || self.assistant_blocks.contains_key(&index)
            || self.closed_blocks.contains(&index)
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let initial_text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let allocation = (!initial_text.is_empty())
                    .then(|| self.begin_text_content())
                    .flatten();
                if !initial_text.is_empty() && allocation.is_none() {
                    return;
                }
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::Text {
                        text: initial_text.clone(),
                        content_index: allocation
                            .as_ref()
                            .map(|(_, _, content_index)| *content_index),
                        output_index: allocation
                            .as_ref()
                            .map(|(_, output_index, _)| *output_index),
                    },
                );
                if let Some((item_id, output_index, content_index)) = allocation {
                    self.emit_text_delta(item_id, output_index, content_index, initial_text);
                }
            }
            Some("tool_use") => {
                if !self.finish_active_text_output() {
                    return;
                }
                let Some(id) = block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let Some(name) = block
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let Some(target) = self
                    .request
                    .as_ref()
                    .and_then(|request| request.state.client_tool(name))
                    .cloned()
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let tool_kind = target.kind;
                let client_name = target.name;
                let client_namespace = target.namespace;
                if self.assistant_blocks.values().any(|existing| {
                    matches!(existing, StreamBlock::Tool { id: existing_id, .. } if existing_id == id)
                }) {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
                if block.get("input").is_some_and(|input| !input.is_object()) {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
                let initial_arguments = block
                    .get("input")
                    .filter(|input| input.as_object().is_some_and(|object| !object.is_empty()))
                    .and_then(|input| serde_json::to_string(input).ok())
                    .unwrap_or_default();
                let output_index = self.next_output_index;
                self.next_output_index = self.next_output_index.saturating_add(1);
                let mut item = match tool_kind {
                    ResponsesToolKind::Function => json!({
                        "id": id,
                        "type": tool_kind.response_item_type(),
                        "status": "in_progress",
                        "call_id": id,
                        "name": client_name,
                        "arguments": "",
                    }),
                    ResponsesToolKind::Custom => json!({
                        "id": custom_tool_item_id(id),
                        "type": tool_kind.response_item_type(),
                        "status": "in_progress",
                        "call_id": id,
                        "name": client_name,
                        "input": "",
                    }),
                };
                if let Some(namespace) = client_namespace.as_ref() {
                    item.as_object_mut()
                        .expect("Responses stream item is an object")
                        .insert("namespace".to_string(), Value::String(namespace.clone()));
                }
                self.frame(
                    "response.output_item.added",
                    json!({
                        "type": "response.output_item.added",
                        "output_index": output_index,
                        "item": item,
                    }),
                );
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::Tool {
                        id: id.to_string(),
                        item_id: if tool_kind == ResponsesToolKind::Custom {
                            custom_tool_item_id(id)
                        } else {
                            id.to_string()
                        },
                        upstream_name: name.to_string(),
                        name: client_name,
                        namespace: client_namespace,
                        kind: tool_kind,
                        arguments: initial_arguments.clone(),
                        output_index,
                    },
                );
                if tool_kind == ResponsesToolKind::Function && !initial_arguments.is_empty() {
                    self.frame(
                        "response.function_call_arguments.delta",
                        json!({
                            "type": "response.function_call_arguments.delta",
                            "response_id": self.response_id.clone(),
                            "item_id": id,
                            "output_index": output_index,
                            "delta": initial_arguments,
                        }),
                    );
                }
            }
            Some("thinking") => {
                let thinking = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let signature = block
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|signature| !signature.is_empty())
                    .map(str::to_string);
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::Thinking {
                        thinking,
                        signature,
                    },
                );
            }
            Some("redacted_thinking") => {
                let Some(data) = block.get("data").and_then(Value::as_str) else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::RedactedThinking {
                        data: data.to_string(),
                    },
                );
            }
            _ => self.fail(AdapterError::upstream_stream_invalid()),
        }
    }

    fn handle_block_delta(&mut self, value: &Value) {
        let Some(index) = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(delta) = value.get("delta").and_then(Value::as_object) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if self.closed_blocks.contains(&index) {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
            let Some(delta) = delta.get("text").and_then(Value::as_str) else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            if delta.is_empty() {
                return;
            }
            let needs_content_part = match self.assistant_blocks.get(&index) {
                Some(StreamBlock::Text { content_index, .. }) => content_index.is_none(),
                _ => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            };
            if needs_content_part {
                let Some((_, output_index, content_index)) = self.begin_text_content() else {
                    return;
                };
                let Some(StreamBlock::Text {
                    content_index: block_content_index,
                    output_index: block_output_index,
                    ..
                }) = self.assistant_blocks.get_mut(&index)
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                *block_content_index = Some(content_index);
                *block_output_index = Some(output_index);
            }
            let (output_index, content_index) = {
                let Some(StreamBlock::Text {
                    text,
                    content_index: Some(content_index),
                    output_index: Some(output_index),
                }) = self.assistant_blocks.get_mut(&index)
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                text.push_str(delta);
                (*output_index, *content_index)
            };
            let Some(text_output) = self
                .text_output
                .as_ref()
                .filter(|output| output.output_index == output_index)
            else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            self.emit_text_delta(
                text_output.item_id.clone(),
                output_index,
                content_index,
                delta.to_string(),
            );
            return;
        }
        let stream_delta = {
            let Some(block) = self.assistant_blocks.get_mut(&index) else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            match (block, delta.get("type").and_then(Value::as_str)) {
                (
                    StreamBlock::Tool {
                        id,
                        kind,
                        arguments,
                        output_index,
                        ..
                    },
                    Some("input_json_delta"),
                ) => {
                    let Some(delta) = delta.get("partial_json").and_then(Value::as_str) else {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    };
                    arguments.push_str(delta);
                    if *kind == ResponsesToolKind::Function {
                        StreamDelta::Tool {
                            item_id: id.clone(),
                            output_index: *output_index,
                            delta: delta.to_string(),
                        }
                    } else {
                        StreamDelta::NoOutput
                    }
                }
                (StreamBlock::Thinking { thinking, .. }, Some("thinking_delta")) => {
                    let Some(delta) = delta.get("thinking").and_then(Value::as_str) else {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    };
                    thinking.push_str(delta);
                    StreamDelta::NoOutput
                }
                (StreamBlock::Thinking { signature, .. }, Some("signature_delta")) => {
                    let Some(delta) = delta.get("signature").and_then(Value::as_str) else {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    };
                    signature.get_or_insert_with(String::new).push_str(delta);
                    StreamDelta::NoOutput
                }
                _ => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            }
        };
        match stream_delta {
            StreamDelta::Tool {
                item_id,
                output_index,
                delta,
            } => {
                let response_id = self.response_id.clone();
                self.frame(
                    "response.function_call_arguments.delta",
                    json!({
                        "type": "response.function_call_arguments.delta",
                        "response_id": response_id,
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": delta,
                    }),
                );
            }
            StreamDelta::NoOutput => {}
        }
    }

    fn handle_block_stop(&mut self, value: &Value) {
        let Some(index) = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if !self.closed_blocks.insert(index) {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let Some(block) = self.assistant_blocks.get(&index).cloned() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        match block {
            StreamBlock::Text {
                text: block_text,
                content_index,
                output_index,
            } => {
                let (Some(content_index), Some(output_index)) = (content_index, output_index)
                else {
                    // Anthropic may emit an empty text block before a tool
                    // block. It has no client-visible Responses equivalent,
                    // so do not manufacture an empty message output item.
                    return;
                };
                let Some(text_output) = self
                    .text_output
                    .as_ref()
                    .filter(|output| output.output_index == output_index)
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let item_id = text_output.item_id.clone();
                let response_id = self.response_id.clone();
                self.frame(
                    "response.output_text.done",
                    json!({
                        "type": "response.output_text.done",
                        "response_id": response_id,
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": content_index,
                        "text": block_text,
                    }),
                );
                self.frame(
                    "response.content_part.done",
                    json!({
                        "type": "response.content_part.done",
                        "response_id": response_id,
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": content_index,
                    }),
                );
            }
            StreamBlock::Tool {
                id,
                item_id,
                name,
                namespace,
                kind,
                arguments: raw_arguments,
                output_index,
                ..
            } => {
                let arguments = if raw_arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    raw_arguments
                };
                let Some(input) = tool_arguments_value(&arguments) else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                match kind {
                    ResponsesToolKind::Function => {
                        let response_id = self.response_id.clone();
                        let mut arguments_done = json!({
                            "type": "response.function_call_arguments.done",
                            "response_id": response_id,
                            "item_id": id,
                            "call_id": id,
                            "name": name,
                            "output_index": output_index,
                            "arguments": arguments,
                        });
                        if let Some(namespace) = namespace.as_ref() {
                            arguments_done
                                .as_object_mut()
                                .expect("Responses function call event is an object")
                                .insert("namespace".to_string(), Value::String(namespace.clone()));
                        }
                        self.frame("response.function_call_arguments.done", arguments_done);
                        let mut item = json!({
                            "id": id,
                            "type": kind.response_item_type(),
                            "status": "completed",
                            "call_id": id,
                            "name": name,
                            "arguments": arguments,
                        });
                        if let Some(namespace) = namespace {
                            item.as_object_mut()
                                .expect("Responses function call item is an object")
                                .insert("namespace".to_string(), Value::String(namespace));
                        }
                        self.frame(
                            "response.output_item.done",
                            json!({
                                "type": "response.output_item.done",
                                "response_id": response_id,
                                "output_index": output_index,
                                "item": item,
                            }),
                        );
                    }
                    ResponsesToolKind::Custom => {
                        let Ok(raw_input) = custom_tool_input(&input) else {
                            self.fail(AdapterError::upstream_stream_invalid());
                            return;
                        };
                        let response_id = self.response_id.clone();
                        self.frame(
                            "response.custom_tool_call_input.done",
                            json!({
                                "type": "response.custom_tool_call_input.done",
                                "response_id": response_id,
                                "item_id": item_id,
                                "output_index": output_index,
                                "input": raw_input,
                            }),
                        );
                        let mut item = json!({
                            "id": item_id,
                            "type": kind.response_item_type(),
                            "status": "completed",
                            "call_id": id,
                            "name": name,
                            "input": raw_input,
                        });
                        if let Some(namespace) = namespace {
                            item.as_object_mut()
                                .expect("Responses custom tool item is an object")
                                .insert("namespace".to_string(), Value::String(namespace));
                        }
                        self.frame(
                            "response.output_item.done",
                            json!({
                                "type": "response.output_item.done",
                                "response_id": response_id,
                                "output_index": output_index,
                                "item": item,
                            }),
                        );
                    }
                }
            }
            StreamBlock::Thinking { .. } | StreamBlock::RedactedThinking { .. } => {}
        }
    }

    fn ensure_text_output(&mut self) -> Option<&mut TextOutput> {
        if self.text_output.is_none() {
            let response_id = self.response_id.as_deref()?;
            let message_index = self.next_message_index;
            self.next_message_index = self.next_message_index.saturating_add(1);
            let item_id = if message_index == 0 {
                format!("msg_{response_id}")
            } else {
                format!("msg_{response_id}_{message_index}")
            };
            let output_index = self.next_output_index;
            self.next_output_index = self.next_output_index.saturating_add(1);
            self.frame(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id,
                        "type": "message",
                        "status": "in_progress",
                        "role": "assistant",
                        "content": [],
                    }
                }),
            );
            self.text_output = Some(TextOutput {
                item_id,
                output_index,
                next_content_index: 0,
            });
        }
        self.text_output.as_mut()
    }

    fn begin_text_content(&mut self) -> Option<(String, usize, usize)> {
        let (item_id, output_index, content_index) = {
            let text_output = self.ensure_text_output()?;
            let content_index = text_output.next_content_index;
            text_output.next_content_index = text_output.next_content_index.saturating_add(1);
            (
                text_output.item_id.clone(),
                text_output.output_index,
                content_index,
            )
        };
        self.frame(
            "response.content_part.added",
            json!({
                "type": "response.content_part.added",
                "item_id": item_id,
                "output_index": output_index,
                "content_index": content_index,
                "part": {"type": "output_text", "text": ""},
            }),
        );
        (!self.terminal).then_some((item_id, output_index, content_index))
    }

    fn emit_text_delta(
        &mut self,
        item_id: String,
        output_index: usize,
        content_index: usize,
        delta: String,
    ) {
        let Some(response_id) = self.response_id.clone() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        self.frame(
            "response.output_text.delta",
            json!({
                "type": "response.output_text.delta",
                "response_id": response_id,
                "item_id": item_id,
                "output_index": output_index,
                "content_index": content_index,
                "delta": delta,
            }),
        );
    }

    fn finish_active_text_output(&mut self) -> bool {
        let Some(text_output) = self.text_output.take() else {
            return true;
        };
        let mut parts = self
            .assistant_blocks
            .iter()
            .filter_map(|(block_index, block)| match block {
                StreamBlock::Text {
                    text,
                    content_index: Some(content_index),
                    output_index: Some(output_index),
                } if *output_index == text_output.output_index => {
                    Some((*block_index, *content_index, text.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if parts.is_empty()
            || parts
                .iter()
                .any(|(block_index, _, _)| !self.closed_blocks.contains(block_index))
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return false;
        }
        parts.sort_by_key(|(_, content_index, _)| *content_index);
        if parts
            .iter()
            .enumerate()
            .any(|(expected, (_, content_index, _))| *content_index != expected)
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return false;
        }
        let content = Value::Array(
            parts
                .into_iter()
                .map(|(_, _, text)| json!({"type": "output_text", "text": text, "annotations": []}))
                .collect(),
        );
        self.frame(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": self.response_id.clone(),
                "output_index": text_output.output_index,
                "item": {
                    "id": text_output.item_id,
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": content,
                }
            }),
        );
        !self.terminal
    }

    fn complete(&mut self) {
        if self.response_id.is_none()
            || self.assistant_blocks.is_empty()
            || self.closed_blocks.len() != self.assistant_blocks.len()
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if !self.finish_active_text_output() {
            return;
        }
        let Some(request) = self.request.take() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(upstream_id) = self.upstream_id.clone() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let content = self
            .assistant_blocks
            .values()
            .map(|block| match block {
                StreamBlock::Text { text, .. } => Ok(json!({"type": "text", "text": text})),
                StreamBlock::Tool {
                    id,
                    upstream_name,
                    arguments,
                    ..
                } => {
                    let input = tool_arguments_value(arguments)
                        .ok_or_else(AdapterError::upstream_stream_invalid)?;
                    Ok(json!({"type": "tool_use", "id": id, "name": upstream_name, "input": input}))
                }
                StreamBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    let mut block = Map::from_iter([
                        ("type".to_string(), Value::String("thinking".to_string())),
                        ("thinking".to_string(), Value::String(thinking.clone())),
                    ]);
                    if let Some(signature) = signature {
                        block.insert("signature".to_string(), Value::String(signature.clone()));
                    }
                    Ok(Value::Object(block))
                }
                StreamBlock::RedactedThinking { data } => {
                    Ok(json!({"type": "redacted_thinking", "data": data}))
                }
            })
            .collect::<AdapterResult<Vec<_>>>();
        let Ok(content) = content else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if validate_messages_tool_calls(&request.state, &content).is_err() {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let (mut output, _) = match responses_output_from_messages_content(&content, &request.state)
        {
            Ok(value) => value,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        let Some(response_id) = self.response_id.clone() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if response_id != bridged_response_id_scoped(request.response_scope(), &upstream_id) {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        set_message_output_id(&mut output, &response_id);
        let response_body = json!({
            "id": response_id,
            "object": "response",
            "created_at": 0,
            "status": "completed",
            "model": self.model,
            "output": output,
            "usage": responses_usage(self.usage.as_ref()),
        });
        let mut continuation = request.state;
        continuation.append_assistant_content(content);
        self.completed = Some(MessagesBridgeResponse {
            response_body: response_body.clone(),
            response_id,
            continuation,
        });
        self.frame(
            "response.completed",
            json!({
                "type": "response.completed",
                "response": response_body,
            }),
        );
        self.terminal = true;
    }

    fn fail(&mut self, error: AdapterError) {
        if self.terminal {
            return;
        }
        let response_id = self
            .response_id
            .clone()
            .unwrap_or_else(|| "resp_bridge_stream_failed".to_string());
        self.frame(
            "response.failed",
            json!({
                "type": "response.failed",
                "response": {
                    "id": response_id,
                    "object": "response",
                    "status": "failed",
                    "model": self.model,
                    "output": [],
                    "error": {
                        "type": "invalid_request_error",
                        "code": error.code(),
                        "message": error.message(),
                    }
                }
            }),
        );
        self.terminal = true;
    }

    fn frame(&mut self, event: &str, payload: Value) {
        let Ok(payload) = serde_json::to_vec(&payload) else {
            self.terminal = true;
            return;
        };
        let mut frame = Vec::with_capacity(event.len() + payload.len() + 20);
        frame.extend_from_slice(b"event: ");
        frame.extend_from_slice(event.as_bytes());
        frame.extend_from_slice(b"\ndata: ");
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(b"\n\n");
        self.output.push_back(frame);
    }
}

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
        if self.terminal {
            return;
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.pending) {
            let event = self.pending.drain(..end).collect::<Vec<_>>();
            self.handle_event(&event);
            if self.terminal {
                self.pending.clear();
                return;
            }
        }
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

    fn handle_event(&mut self, event: &[u8]) {
        if sse_done(event) {
            if self.finished_upstream {
                self.complete();
            } else {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        }
        let Some(value) = parse_sse_data(event) else {
            if sse_event_has_data(event) {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        if value.get("error").is_some() {
            self.upstream_error = Some(value);
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if let Some(usage) = value.get("usageMetadata") {
            self.usage = Some(usage.clone());
        }
        let Some(candidate) = value
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return;
        };
        let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        else {
            if candidate
                .get("finishReason")
                .and_then(Value::as_str)
                .is_some()
            {
                self.finished_upstream = true;
                self.complete();
            } else {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        for (part_index, part) in parts.iter().enumerate() {
            let Some(part) = part.as_object() else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            if let Some(value) = part.get("text").and_then(Value::as_str) {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    self.ensure_started();
                    if self.terminal {
                        return;
                    }
                    let delta = incremental_delta(&self.thinking, value);
                    if !delta.is_empty() {
                        if self.thinking.is_empty() {
                            self.order.push(GeminiStreamOutput::Thinking);
                            self.thinking_output_index = Some(self.next_output_index);
                            self.next_output_index = self.next_output_index.saturating_add(1);
                            self.frame(
                                "response.output_item.added",
                                json!({"type":"response.output_item.added","output_index":self.thinking_output_index,"item":{"id":format!("reasoning_{}_0",self.request.response_id()),"type":"reasoning","status":"in_progress","summary":[]}}),
                            );
                        }
                        self.thinking.push_str(&delta);
                        self.frame(
                            "response.reasoning_summary_text.delta",
                            json!({
                                "type": "response.reasoning_summary_text.delta",
                                "response_id": self.request.response_id(),
                                "output_index": self.thinking_output_index,
                                "delta": delta,
                            }),
                        );
                    }
                } else {
                    self.ensure_started();
                    if self.terminal {
                        return;
                    }
                    let delta = incremental_delta(&self.text, value);
                    if !delta.is_empty() {
                        if self.text.is_empty() {
                            self.order.push(GeminiStreamOutput::Text);
                        }
                        self.ensure_text_output();
                        if self.terminal {
                            return;
                        }
                        self.text.push_str(&delta);
                        self.frame(
                            "response.output_text.delta",
                            json!({
                                "type": "response.output_text.delta",
                                "response_id": self.request.response_id(),
                                "item_id": format!("msg_{}_{}", self.request.response_id(), self.text_output_index),
                                "output_index": self.text_output_index,
                                "content_index": 0,
                                "delta": delta,
                            }),
                        );
                    }
                }
                continue;
            }
            if let Some(call) = part.get("functionCall").and_then(Value::as_object) {
                self.handle_function_call(part_index, part, call);
                if self.terminal {
                    return;
                }
                continue;
            }
            if part.get("thoughtSignature").is_some() {
                if let Some(call) = self.calls.values_mut().next_back() {
                    call.thought_signature = part
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                continue;
            }
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(|reason| {
                self.finish_reason = Some(reason.to_string());
                true
            })
            .unwrap_or(false)
        {
            self.finished_upstream = true;
            self.complete();
        }
    }

    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        self.frame(
            "response.created",
            json!({
                "type": "response.created",
                "response": {
                    "id": self.request.response_id(),
                    "object": "response",
                    "status": "in_progress",
                    "model": self.request.model(),
                    "output": [],
                }
            }),
        );
    }

    fn ensure_text_output(&mut self) {
        if self.text_started {
            return;
        }
        self.text_started = true;
        self.text_output_index = self.next_output_index;
        self.next_output_index = self.next_output_index.saturating_add(1);
        let item_id = format!(
            "msg_{}_{}",
            self.request.response_id(),
            self.text_output_index
        );
        self.frame(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":self.text_output_index,"item":{
                "id": item_id, "type":"message",
                "status":"in_progress","role":"assistant","content":[]
            }}),
        );
        self.frame(
            "response.content_part.added",
            json!({"type":"response.content_part.added","item_id":format!("msg_{}_{}", self.request.response_id(), self.text_output_index),
                "output_index":self.text_output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        );
    }

    fn handle_function_call(
        &mut self,
        part_index: usize,
        part: &Map<String, Value>,
        call: &Map<String, Value>,
    ) {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let key = if let Some(call_id) = call_id {
            self.calls
                .iter()
                .find_map(|(key, state)| (state.id == call_id).then_some(*key))
        } else {
            None
        }
        .or_else(|| {
            name.and_then(|name| {
                self.calls.iter().find_map(|(key, state)| {
                    (state.name == name && *key == part_index).then_some(*key)
                })
            })
        })
        .or_else(|| {
            self.active_call.filter(|active| {
                name.is_none()
                    || self
                        .calls
                        .get(active)
                        .is_some_and(|state| Some(state.name.as_str()) == name)
            })
        })
        .or_else(|| {
            name.map(|_| {
                if self.calls.contains_key(&part_index) {
                    self.calls
                        .keys()
                        .next_back()
                        .copied()
                        .unwrap_or(part_index)
                        .saturating_add(1)
                } else {
                    part_index
                }
            })
        });
        let Some(key) = key else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let name = name
            .map(str::to_string)
            .or_else(|| self.calls.get(&key).map(|state| state.name.clone()));
        let Some(name) = name else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(target) = self.request.state().client_tool(&name) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let target_kind = target.kind;
        let target_name = target.name.clone();
        let target_namespace = target.namespace.clone();
        let is_new = !self.calls.contains_key(&key);
        if is_new {
            let args = match function_call_args(call) {
                Ok(args) => args,
                Err(_) => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            };
            self.ensure_started();
            let output_index = self.next_output_index;
            self.next_output_index = self.next_output_index.saturating_add(1);
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{}_{}", self.request.response_id(), key));
            let item_id = if target_kind == ResponsesToolKind::Custom {
                custom_tool_item_id(&id)
            } else {
                id.clone()
            };
            self.calls.insert(
                key,
                GeminiStreamCall {
                    id,
                    item_id,
                    name,
                    kind: target_kind,
                    args,
                    output_index,
                    thought_signature: part
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    emitted_arguments: String::new(),
                },
            );
            self.order.push(GeminiStreamOutput::Call(key));
        }
        let args = if is_new {
            self.calls
                .get(&key)
                .map(|state| state.args.clone())
                .unwrap_or_else(|| json!({}))
        } else {
            let mut args = self
                .calls
                .get(&key)
                .map(|state| state.args.clone())
                .unwrap_or_else(|| json!({}));
            if let Some(full_args) = call.get("args") {
                if !full_args.is_object() {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
                args = full_args.clone();
            }
            if let Some(partial_args) = call.get("partialArgs") {
                if apply_partial_args(&mut args, partial_args).is_err() {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            }
            args
        };
        if let Some(call_state) = self.calls.get_mut(&key) {
            call_state.args = args.clone();
            if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
                call_state.thought_signature = Some(signature.to_string());
            }
        }
        if is_new {
            let call_state = self.calls.get(&key).expect("inserted Gemini call").clone();
            let mut item = if call_state.kind == ResponsesToolKind::Custom {
                json!({"id":call_state.item_id,"type":"custom_tool_call","status":"in_progress","call_id":call_state.id,"name":target_name,"input":""})
            } else {
                json!({"id":call_state.id,"type":"function_call","status":"in_progress","call_id":call_state.id,"name":target_name,"arguments":""})
            };
            if let Some(namespace) = target_namespace {
                item["namespace"] = Value::String(namespace);
            }
            self.frame("response.output_item.added", json!({"type":"response.output_item.added","output_index":call_state.output_index,"item":item}));
            if call_state.kind == ResponsesToolKind::Function {
                self.emit_call_arguments_delta(key);
            }
        }
        if let Some(call_state) = self.calls.get(&key) {
            if !is_new && call_state.kind == ResponsesToolKind::Function {
                self.emit_call_arguments_delta(key);
            }
        }
        if call.get("willContinue").and_then(Value::as_bool) == Some(true) {
            self.active_call = Some(key);
        } else if self.active_call == Some(key) {
            self.active_call = None;
        }
    }

    fn emit_call_arguments_delta(&mut self, key: usize) {
        let Some(call_state) = self.calls.get_mut(&key) else {
            return;
        };
        let Ok(arguments) = serde_json::to_string(&call_state.args) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if call_state.emitted_arguments.is_empty()
            && call_state.args.as_object().is_some_and(Map::is_empty)
        {
            return;
        }
        let delta = incremental_delta(&call_state.emitted_arguments, &arguments);
        if delta.is_empty() {
            return;
        }
        call_state.emitted_arguments = arguments;
        let item_id = call_state.item_id.clone();
        let output_index = call_state.output_index;
        let response_id = self.request.response_id().to_string();
        self.frame(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","response_id":response_id,"item_id":item_id,"output_index":output_index,"delta":delta}),
        );
    }

    fn complete(&mut self) {
        if self.terminal {
            return;
        }
        if !self.started
            || (self.text.is_empty() && self.thinking.is_empty() && self.calls.is_empty())
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let mut parts = Vec::new();
        for item in &self.order {
            match item {
                GeminiStreamOutput::Thinking if !self.thinking.is_empty() => {
                    parts.push(json!({"thought":true,"text":self.thinking}));
                }
                GeminiStreamOutput::Text if !self.text.is_empty() => {
                    parts.push(json!({"text":self.text}));
                }
                GeminiStreamOutput::Call(key) => {
                    if let Some(call) = self.calls.get(key) {
                        let mut function_call =
                            json!({"name":call.name,"args":call.args,"id":call.id});
                        if let Some(signature) = call.thought_signature.as_ref() {
                            function_call["thoughtSignature"] = Value::String(signature.clone());
                        }
                        parts.push(json!({"functionCall":function_call}));
                    }
                }
                _ => {}
            }
        }
        let upstream = json!({"candidates":[{"content":{"parts":parts},"finishReason":self.finish_reason}],"usageMetadata":self.usage.clone()});
        let response =
            match super::gemini::translate_gemini_response(self.request.clone(), &upstream) {
                Ok(response) => response,
                Err(error) => {
                    self.fail(error);
                    return;
                }
            };
        for (output_index, item) in response.response_body["output"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            match item.get("type").and_then(Value::as_str) {
                Some("function_call") => self.frame(
                    "response.function_call_arguments.done",
                    json!({"type":"response.function_call_arguments.done","response_id":self.request.response_id(),"item_id":item["id"],"call_id":item["call_id"],"name":item["name"],"output_index":output_index,"arguments":item["arguments"]}),
                ),
                Some("custom_tool_call") => self.frame(
                    "response.custom_tool_call_input.done",
                    json!({"type":"response.custom_tool_call_input.done","response_id":self.request.response_id(),"item_id":item["id"],"output_index":output_index,"input":item["input"]}),
                ),
                Some("message") => {
                    self.frame(
                        "response.output_text.done",
                        json!({"type":"response.output_text.done","response_id":self.request.response_id(),"item_id":item["id"],"output_index":output_index,"content_index":0,"text":item["content"][0]["text"]}),
                    );
                    self.frame(
                        "response.content_part.done",
                        json!({"type":"response.content_part.done","response_id":self.request.response_id(),"item_id":item["id"],"output_index":output_index,"content_index":0}),
                    );
                }
                _ => {}
            }
            self.frame("response.output_item.done", json!({"type":"response.output_item.done","response_id":self.request.response_id(),"output_index":output_index,"item":item}));
        }
        self.completed = Some(MessagesBridgeResponse {
            response_body: response.response_body.clone(),
            response_id: response.response_id.clone(),
            continuation: response.continuation,
        });
        self.frame(
            "response.completed",
            json!({"type": "response.completed", "response": response.response_body}),
        );
        self.terminal = true;
    }

    fn fail(&mut self, error: AdapterError) {
        if self.terminal {
            return;
        }
        self.frame(
            "response.failed",
            json!({
                "type": "response.failed",
                "response": {
                    "id": self.request.response_id(),
                    "object": "response",
                    "status": "failed",
                    "model": self.request.model(),
                    "output": [],
                    "error": {
                        "type": "invalid_request_error",
                        "code": error.code(),
                        "message": error.message(),
                    }
                }
            }),
        );
        self.terminal = true;
    }

    fn frame(&mut self, event: &str, payload: Value) {
        let Ok(payload) = serde_json::to_vec(&payload) else {
            self.terminal = true;
            return;
        };
        let mut frame = Vec::with_capacity(event.len() + payload.len() + 20);
        frame.extend_from_slice(b"event: ");
        frame.extend_from_slice(event.as_bytes());
        frame.extend_from_slice(b"\ndata: ");
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(b"\n\n");
        self.output.push_back(frame);
    }
}

fn sse_done(event: &[u8]) -> bool {
    event.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.strip_prefix(b"data:")
            .map(|value| value.trim_ascii() == b"[DONE]")
            .unwrap_or(false)
    })
}

fn incremental_delta(previous: &str, incoming: &str) -> String {
    incoming
        .strip_prefix(previous)
        .unwrap_or(incoming)
        .to_string()
}

fn parse_sse_data(event: &[u8]) -> Option<Value> {
    let mut data = Vec::new();
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(value) = line.strip_prefix(b"data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
    }
    (!data.is_empty())
        .then(|| serde_json::from_slice(&data).ok())
        .flatten()
}

fn sse_event_has_data(event: &[u8]) -> bool {
    event.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.strip_prefix(b"data:")
            .is_some_and(|value| value.iter().any(|byte| !byte.is_ascii_whitespace()))
    })
}

fn is_ignorable_metadata_event(kind: &str) -> bool {
    matches!(
        kind,
        "message_metadata" | "content_block_metadata" | "citation" | "message_citation"
    ) || kind.ends_with("_metadata")
        || kind.ends_with("_citation")
}

fn tool_arguments_value(arguments: &str) -> Option<Value> {
    if arguments.trim().is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod gemini_stream_tests {
    use super::*;
    use crate::protocol::adapter::gemini::prepare_responses_to_gemini;

    #[test]
    fn gemini_stream_emits_responses_events_and_usage() {
        let request = prepare_responses_to_gemini(
            &json!({"input": "Hello"}),
            "gemini-test",
            true,
            "route",
            "request-42",
        )
        .unwrap();
        let mut bridge = GeminiStreamBridge::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}

data: {"candidates":[{"content":{"parts":[{"text":"Hello world"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5}}

"#,
        );
        let output = std::iter::from_fn(|| bridge.pop_output())
            .flat_map(|frame| {
                String::from_utf8(frame)
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(bridge.is_terminal());
        assert!(output.contains("response.output_text.delta"));
        assert!(output.contains("Hello world"));
        assert!(output.contains("\"total_tokens\":5"));
    }

    #[test]
    fn gemini_stream_emits_tool_events_and_captures_continuation() {
        let request = prepare_responses_to_gemini(
            &json!({
                "input": "inspect",
                "tools": [{"type":"function","name":"run","parameters":{"type":"object"}}]
            }),
            "gemini-test",
            true,
            "route",
            "request-tool",
        )
        .unwrap();
        let mut bridge = GeminiStreamBridge::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"run","args":{"command":"pwd"},"id":"call-1"}}]}}]}

data: {"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":2}}

"#,
        );
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");
        assert!(bridge.completed().is_some());
        assert!(output.contains("response.function_call_arguments.delta"));
        assert!(output.contains("response.function_call_arguments.done"));
        assert_eq!(
            bridge.completed().unwrap().response_body["output"][0]["type"],
            "function_call"
        );
        assert_eq!(bridge.completed().unwrap().continuation.messages.len(), 2);
    }

    #[test]
    fn gemini_stream_reassembles_vertex_partial_function_arguments() {
        let request = prepare_responses_to_gemini(
            &json!({
                "input": "weather",
                "tools": [{
                    "type": "function",
                    "name": "get_weather",
                    "parameters": {"type":"object"}
                }]
            }),
            "gemini-test",
            true,
            "route",
            "partial-tool",
        )
        .unwrap();
        let mut bridge = GeminiStreamBridge::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","willContinue":true},"thoughtSignature":"sig"}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.location","stringValue":"Paris","willContinue":true}],"willContinue":true}}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.unit","stringValue":"C","willContinue":true}],"willContinue":true}}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3}}

"#,
        );
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");
        let completed = bridge.completed().expect("partial tool should complete");
        assert!(
            output
                .matches("response.function_call_arguments.delta")
                .count()
                >= 2
        );
        assert_eq!(
            completed.response_body["output"][0]["arguments"],
            r#"{"location":"Paris","unit":"C"}"#
        );
        assert_eq!(
            completed.continuation.messages[1]["parts"][0]["functionCall"]["thoughtSignature"],
            "sig"
        );
    }
}

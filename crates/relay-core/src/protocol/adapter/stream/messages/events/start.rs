use super::super::super::super::contracts::{custom_tool_item_id, AdapterError, ResponsesToolKind};
use super::super::{MessagesStreamBridge, StreamBlock};
use serde_json::{json, Map, Value};

impl MessagesStreamBridge {
    pub(in crate::protocol::adapter::stream::messages::events) fn handle_block_start(
        &mut self,
        value: &Value,
    ) {
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
            Some("text") => self.start_text_block(index, block),
            Some("tool_use") => self.start_tool_use_block(index, block),
            Some("thinking") => self.start_thinking_block(index, block),
            Some("redacted_thinking") => self.start_redacted_thinking_block(index, block),
            _ => self.fail(AdapterError::upstream_stream_invalid()),
        }
    }

    fn start_text_block(&mut self, index: usize, block: &Map<String, Value>) {
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

    fn start_tool_use_block(&mut self, index: usize, block: &Map<String, Value>) {
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

    fn start_thinking_block(&mut self, index: usize, block: &Map<String, Value>) {
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

    fn start_redacted_thinking_block(&mut self, index: usize, block: &Map<String, Value>) {
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
}

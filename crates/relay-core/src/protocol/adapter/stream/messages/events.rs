use super::super::super::contracts::{
    AdapterError, AdapterResult, MessagesBridgeRequest, MessagesBridgeResponse,
};
use super::super::super::messages::{
    bridged_response_id_scoped, responses_output_from_messages_content, responses_usage,
    set_message_output_id, validate_messages_tool_calls,
};
use super::super::frame::{
    failed_responses_event, is_ignorable_metadata_event, merge_usage, parse_sse_data,
    push_sse_frame, sse_event_has_data, tool_arguments_value,
};
use super::{MessagesStreamBridge, StreamBlock, TextOutput};
use serde_json::{json, Map, Value};

mod delta;
mod start;
mod stop;

impl MessagesStreamBridge {
    pub(super) fn handle_event(&mut self, event: &[u8]) {
        let Some(event_payload) = parse_sse_data(event) else {
            if sse_event_has_data(event) {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        let Some(kind) = event_payload.get("type").and_then(Value::as_str) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        match kind {
            "message_start" => self.handle_message_start(&event_payload),
            "content_block_start" => self.handle_block_start(&event_payload),
            "content_block_delta"
                if event_payload
                    .get("delta")
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|delta| {
                        matches!(delta, "citations_delta" | "document" | "compaction_delta")
                    }) => {}
            "content_block_delta" => self.handle_block_delta(&event_payload),
            "content_block_stop" => self.handle_block_stop(&event_payload),
            "message_delta" => {
                if let Some(reason) = event_payload
                    .pointer("/delta/stop_reason")
                    .filter(|reason| !reason.is_null())
                {
                    let Some(reason) = reason.as_str() else {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    };
                    if super::super::super::messages::messages_response_terminal(Some(reason))
                        .is_err()
                    {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    }
                    self.stop_reason = Some(reason.to_owned());
                }
                if let Some(usage) = event_payload.get("usage") {
                    merge_usage(&mut self.usage, usage);
                }
            }
            "message_stop" => self.complete(),
            // Anthropic may emit keep-alives and metadata deltas which do not
            // change the client-visible Responses output. They must not turn
            // an otherwise valid stream into a synthetic adapter failure.
            "ping" => {}
            "error" => {
                self.upstream_error = Some(event_payload.clone());
                self.fail(AdapterError::upstream_stream_invalid());
            }
            kind if is_ignorable_metadata_event(kind) => {}
            _ => self.fail(AdapterError::upstream_stream_invalid()),
        }
    }

    fn handle_message_start(&mut self, event_payload: &Value) {
        let Some(message) = event_payload.get("message") else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(upstream_id) = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message_id| !message_id.is_empty())
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
            merge_usage(&mut self.usage, usage);
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

    fn completed_message_content(&self) -> AdapterResult<Vec<Value>> {
        self.assistant_blocks
            .values()
            .map(|block| match block {
                StreamBlock::Text { text, .. } => Ok(json!({"type": "text", "text": text})),
                StreamBlock::Tool {
                    id,
                    upstream_name,
                    arguments,
                    ..
                } => {
                    let tool_input = tool_arguments_value(arguments)
                        .ok_or_else(AdapterError::upstream_stream_invalid)?;
                    Ok(json!({"type": "tool_use", "id": id, "name": upstream_name, "input": tool_input}))
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
            .collect::<AdapterResult<Vec<_>>>()
    }

    fn complete(&mut self) {
        if self.response_id.is_none()
            || (self.assistant_blocks.is_empty()
                && !matches!(self.stop_reason.as_deref(), Some("refusal" | "max_tokens")))
            || self.closed_blocks.len() != self.assistant_blocks.len()
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if !self.finish_active_text_output() {
            return;
        }
        let (status, incomplete_reason) =
            match super::super::super::messages::messages_response_terminal(
                self.stop_reason.as_deref(),
            ) {
                Ok(terminal) => terminal,
                Err(_) => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            };
        let Some(request) = self.request.take() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(upstream_id) = self.upstream_id.clone() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let content = self.completed_message_content();
        let Ok(content) = content else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if validate_messages_tool_calls(&request.bridge_state, &content).is_err() {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let (mut output, _) = match responses_output_from_messages_content(
            &content,
            &request.bridge_state,
            status == "incomplete",
        ) {
            Ok(output_parts) => output_parts,
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
            "status": status,
            "incomplete_details": incomplete_reason.map(|reason| json!({"reason": reason})),
            "model": self.model,
            "output": output,
            "usage": responses_usage(self.usage.as_ref()),
        });
        let mut continuation = request.bridge_state;
        continuation.append_assistant_content(content);
        self.completed = Some(MessagesBridgeResponse {
            response_body: response_body.clone(),
            response_id,
            continuation,
        });
        let event = if status == "completed" {
            "response.completed"
        } else {
            "response.incomplete"
        };
        self.frame(
            event,
            json!({
                "type": event,
                "response": response_body,
            }),
        );
        self.terminal = true;
    }

    pub(super) fn fail(&mut self, error: AdapterError) {
        if self.terminal {
            return;
        }
        let response_id = self
            .response_id
            .clone()
            .unwrap_or_else(|| "resp_bridge_stream_failed".to_string());
        self.frame(
            "response.failed",
            failed_responses_event(&response_id, &self.model, error.code(), error.message()),
        );
        self.terminal = true;
    }

    fn frame(&mut self, event: &str, event_payload: Value) {
        if !push_sse_frame(&mut self.output, event, &event_payload) {
            self.terminal = true;
        }
    }
}

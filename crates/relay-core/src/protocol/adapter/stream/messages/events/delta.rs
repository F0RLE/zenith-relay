use super::super::super::super::contracts::{AdapterError, ResponsesToolKind};
use super::super::{MessagesStreamBridge, StreamBlock, StreamDelta};
use serde_json::{json, Value};

impl MessagesStreamBridge {
    pub(in crate::protocol::adapter::stream::messages::events) fn handle_block_delta(
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
}

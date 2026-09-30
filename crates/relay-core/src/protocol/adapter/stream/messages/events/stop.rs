use super::super::super::super::contracts::{AdapterError, ResponsesToolKind};
use super::super::super::super::messages::custom_tool_input;
use super::super::super::frame::tool_arguments_value;
use super::super::{MessagesStreamBridge, StreamBlock};
use serde_json::{json, Value};

struct FinishedToolBlock {
    id: String,
    item_id: String,
    name: String,
    namespace: Option<String>,
    kind: ResponsesToolKind,
    raw_arguments: String,
    output_index: usize,
}

impl MessagesStreamBridge {
    pub(in crate::protocol::adapter::stream::messages::events) fn handle_block_stop(
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
                text,
                content_index,
                output_index,
            } => self.finish_text_block(text, content_index, output_index),
            StreamBlock::Tool {
                id,
                item_id,
                name,
                namespace,
                kind,
                arguments,
                output_index,
                ..
            } => self.finish_tool_block(FinishedToolBlock {
                id,
                item_id,
                name,
                namespace,
                kind,
                raw_arguments: arguments,
                output_index,
            }),
            StreamBlock::Thinking { .. } | StreamBlock::RedactedThinking { .. } => {}
        }
    }

    fn finish_text_block(
        &mut self,
        block_text: String,
        content_index: Option<usize>,
        output_index: Option<usize>,
    ) {
        let (Some(content_index), Some(output_index)) = (content_index, output_index) else {
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

    fn finish_tool_block(&mut self, block: FinishedToolBlock) {
        let FinishedToolBlock {
            id,
            item_id,
            name,
            namespace,
            kind,
            raw_arguments,
            output_index,
        } = block;
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
                self.finish_function_call(id, name, namespace, kind, arguments, output_index)
            }
            ResponsesToolKind::Custom => self.finish_custom_tool_call(
                FinishedToolBlock {
                    id,
                    item_id,
                    name,
                    namespace,
                    kind,
                    raw_arguments: arguments,
                    output_index,
                },
                input,
            ),
        }
    }

    fn finish_function_call(
        &mut self,
        id: String,
        name: String,
        namespace: Option<String>,
        kind: ResponsesToolKind,
        arguments: String,
        output_index: usize,
    ) {
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

    fn finish_custom_tool_call(&mut self, block: FinishedToolBlock, input: Value) {
        let FinishedToolBlock {
            id,
            item_id,
            name,
            namespace,
            kind,
            output_index,
            ..
        } = block;
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

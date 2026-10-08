use crate::protocol::AdapterError;
use serde_json::{Map, Value};

/// Basis Points may return a completed JSON response even when a caller asked
/// for SSE. Emit the standard terminal Responses events so clients keep their
/// normal stream contract without exposing the internal transport.
pub(in crate::gateway::execution) fn synthetic_stream(
    response_body: &[u8],
) -> Result<Vec<u8>, AdapterError> {
    let completed_response: Value = serde_json::from_slice(response_body)
        .map_err(|_| AdapterError::upstream_stream_invalid())?;
    let terminal_event = match completed_response.get("status").and_then(Value::as_str) {
        Some("completed") => "response.completed",
        Some("incomplete") => "response.incomplete",
        _ => return Err(AdapterError::upstream_stream_invalid()),
    };
    if completed_response
        .get("output")
        .and_then(Value::as_array)
        .is_none()
    {
        return Err(AdapterError::upstream_stream_invalid());
    }
    let mut created_response = completed_response.clone();
    if let Some(response_object) = created_response.as_object_mut() {
        response_object.insert(
            "status".to_string(),
            Value::String("in_progress".to_string()),
        );
        response_object.insert("output".to_string(), Value::Array(Vec::new()));
    }
    let mut sequence = 0_u64;
    let mut sse_body = String::new();
    {
        let mut cursor = EventCursor {
            sse_body: &mut sse_body,
            sequence: &mut sequence,
        };
        cursor.emit(
            "response.created",
            Map::from_iter([(String::from("response"), created_response.clone())]),
        );
        cursor.emit(
            "response.in_progress",
            Map::from_iter([(String::from("response"), created_response)]),
        );
        if let Some(output_items) = completed_response.get("output").and_then(Value::as_array) {
            for (index, output_item) in output_items.iter().enumerate() {
                emit_output_item(&mut cursor, index, output_item);
            }
        }
        cursor.emit(
            terminal_event,
            Map::from_iter([(String::from("response"), completed_response)]),
        );
    }
    sse_body.push_str("data: [DONE]\n\n");
    Ok(sse_body.into_bytes())
}

struct EventCursor<'a> {
    sse_body: &'a mut String,
    sequence: &'a mut u64,
}

impl EventCursor<'_> {
    fn emit(&mut self, event: &str, mut event_fields: Map<String, Value>) {
        event_fields.insert("type".to_string(), Value::String(event.to_string()));
        event_fields.insert(
            "sequence_number".to_string(),
            Value::Number((*self.sequence).into()),
        );
        *self.sequence += 1;
        self.sse_body.push_str("event: ");
        self.sse_body.push_str(event);
        self.sse_body.push_str("\ndata: ");
        self.sse_body.push_str(
            &serde_json::to_string(&Value::Object(event_fields))
                .unwrap_or_else(|_| "{}".to_string()),
        );
        self.sse_body.push_str("\n\n");
    }
}

fn emit_output_item(cursor: &mut EventCursor<'_>, index: usize, output_item: &Value) {
    let Some(output_object) = output_item.as_object() else {
        return;
    };
    let output_index = Value::Number((index as u64).into());
    let item_id = output_object.get("id").cloned().unwrap_or(Value::Null);
    let item_type = output_object.get("type").and_then(Value::as_str);
    if item_type == Some("message") {
        emit_message(cursor, &output_index, &item_id, output_object);
    } else {
        emit_call(cursor, &output_index, &item_id, output_object, item_type);
    }
    cursor.emit(
        "response.output_item.done",
        Map::from_iter([
            (String::from("output_index"), output_index),
            (String::from("item"), output_item.clone()),
        ]),
    );
}

fn emit_message(
    cursor: &mut EventCursor<'_>,
    output_index: &Value,
    item_id: &Value,
    message_item: &Map<String, Value>,
) {
    let mut added_item = message_item.clone();
    added_item.insert(
        "status".to_string(),
        Value::String("in_progress".to_string()),
    );
    added_item.insert("content".to_string(), Value::Array(Vec::new()));
    cursor.emit(
        "response.output_item.added",
        Map::from_iter([
            (String::from("output_index"), output_index.clone()),
            (String::from("item"), Value::Object(added_item)),
        ]),
    );
    let Some(content_blocks) = message_item.get("content").and_then(Value::as_array) else {
        return;
    };
    for (content_index, content_part) in content_blocks.iter().enumerate() {
        let Some(part_object) = content_part.as_object() else {
            continue;
        };
        if part_object.get("type").and_then(Value::as_str) != Some("output_text") {
            continue;
        }
        emit_text_part(
            cursor,
            output_index,
            item_id,
            content_index,
            content_part,
            part_object,
        );
    }
}

fn emit_text_part(
    cursor: &mut EventCursor<'_>,
    output_index: &Value,
    item_id: &Value,
    content_index: usize,
    content_part: &Value,
    content_object: &Map<String, Value>,
) {
    let text = content_object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content_index = Value::Number((content_index as u64).into());
    let mut empty_part = content_object.clone();
    empty_part.insert("text".to_string(), Value::String(String::new()));
    cursor.emit(
        "response.content_part.added",
        Map::from_iter([
            (String::from("output_index"), output_index.clone()),
            (String::from("item_id"), item_id.clone()),
            (String::from("content_index"), content_index.clone()),
            (String::from("part"), Value::Object(empty_part)),
        ]),
    );
    if !text.is_empty() {
        cursor.emit(
            "response.output_text.delta",
            Map::from_iter([
                (String::from("output_index"), output_index.clone()),
                (String::from("item_id"), item_id.clone()),
                (String::from("content_index"), content_index.clone()),
                (String::from("delta"), Value::String(text.to_string())),
            ]),
        );
    }
    cursor.emit(
        "response.output_text.done",
        Map::from_iter([
            (String::from("output_index"), output_index.clone()),
            (String::from("item_id"), item_id.clone()),
            (String::from("content_index"), content_index.clone()),
            (String::from("text"), Value::String(text.to_string())),
        ]),
    );
    cursor.emit(
        "response.content_part.done",
        Map::from_iter([
            (String::from("output_index"), output_index.clone()),
            (String::from("item_id"), item_id.clone()),
            (String::from("content_index"), content_index),
            (String::from("part"), content_part.clone()),
        ]),
    );
}

fn emit_call(
    cursor: &mut EventCursor<'_>,
    output_index: &Value,
    item_id: &Value,
    tool_call_item: &Map<String, Value>,
    item_type: Option<&str>,
) {
    let mut added_item = tool_call_item.clone();
    let argument_field = match item_type {
        Some("function_call") => Some("arguments"),
        Some("custom_tool_call") => Some("input"),
        _ => None,
    };
    if let Some(field) = argument_field {
        added_item.insert(field.to_string(), Value::String(String::new()));
    }
    cursor.emit(
        "response.output_item.added",
        Map::from_iter([
            (String::from("output_index"), output_index.clone()),
            (String::from("item"), Value::Object(added_item)),
        ]),
    );
    if let Some(field) = argument_field {
        if let Some(argument_text) = tool_call_item.get(field).and_then(Value::as_str) {
            if !argument_text.is_empty() {
                let event = if field == "arguments" {
                    "response.function_call_arguments.delta"
                } else {
                    "response.custom_tool_call_input.delta"
                };
                cursor.emit(
                    event,
                    Map::from_iter([
                        (String::from("output_index"), output_index.clone()),
                        (String::from("item_id"), item_id.clone()),
                        (
                            String::from("delta"),
                            Value::String(argument_text.to_string()),
                        ),
                    ]),
                );
            }
            let event = if field == "arguments" {
                "response.function_call_arguments.done"
            } else {
                "response.custom_tool_call_input.done"
            };
            cursor.emit(
                event,
                Map::from_iter([
                    (String::from("output_index"), output_index.clone()),
                    (String::from("item_id"), item_id.clone()),
                    (
                        String::from(field),
                        Value::String(argument_text.to_string()),
                    ),
                ]),
            );
        }
    }
}

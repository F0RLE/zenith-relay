use crate::protocol::AdapterError;
use serde_json::{Map, Value};

/// Basis Points may return a completed JSON response even when a caller asked
/// for SSE. Emit the standard terminal Responses events so clients keep their
/// normal stream contract without exposing the internal transport.
pub(in crate::gateway::execution) fn synthetic_stream(
    body: &[u8],
) -> Result<Vec<u8>, AdapterError> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AdapterError::upstream_stream_invalid())?;
    let terminal_event = match response.get("status").and_then(Value::as_str) {
        Some("completed") => "response.completed",
        Some("incomplete") => "response.incomplete",
        _ => return Err(AdapterError::upstream_stream_invalid()),
    };
    if response.get("output").and_then(Value::as_array).is_none() {
        return Err(AdapterError::upstream_stream_invalid());
    }
    let mut created = response.clone();
    if let Some(object) = created.as_object_mut() {
        object.insert(
            "status".to_string(),
            Value::String("in_progress".to_string()),
        );
        object.insert("output".to_string(), Value::Array(Vec::new()));
    }
    let mut sequence = 0_u64;
    let mut result = String::new();
    {
        let mut cursor = EventCursor {
            result: &mut result,
            sequence: &mut sequence,
        };
        cursor.emit(
            "response.created",
            Map::from_iter([(String::from("response"), created.clone())]),
        );
        cursor.emit(
            "response.in_progress",
            Map::from_iter([(String::from("response"), created)]),
        );
        if let Some(items) = response.get("output").and_then(Value::as_array) {
            for (index, item) in items.iter().enumerate() {
                emit_output_item(&mut cursor, index, item);
            }
        }
        cursor.emit(
            terminal_event,
            Map::from_iter([(String::from("response"), response)]),
        );
    }
    result.push_str("data: [DONE]\n\n");
    Ok(result.into_bytes())
}

struct EventCursor<'a> {
    result: &'a mut String,
    sequence: &'a mut u64,
}

impl EventCursor<'_> {
    fn emit(&mut self, event: &str, mut payload: Map<String, Value>) {
        payload.insert("type".to_string(), Value::String(event.to_string()));
        payload.insert(
            "sequence_number".to_string(),
            Value::Number((*self.sequence).into()),
        );
        *self.sequence += 1;
        self.result.push_str("event: ");
        self.result.push_str(event);
        self.result.push_str("\ndata: ");
        self.result.push_str(
            &serde_json::to_string(&Value::Object(payload)).unwrap_or_else(|_| "{}".to_string()),
        );
        self.result.push_str("\n\n");
    }
}

fn emit_output_item(cursor: &mut EventCursor<'_>, index: usize, item: &Value) {
    let Some(object) = item.as_object() else {
        return;
    };
    let output_index = Value::Number((index as u64).into());
    let item_id = object.get("id").cloned().unwrap_or(Value::Null);
    let item_type = object.get("type").and_then(Value::as_str);
    if item_type == Some("message") {
        emit_message(cursor, &output_index, &item_id, object);
    } else {
        emit_call(cursor, &output_index, &item_id, object, item_type);
    }
    cursor.emit(
        "response.output_item.done",
        Map::from_iter([
            (String::from("output_index"), output_index),
            (String::from("item"), item.clone()),
        ]),
    );
}

fn emit_message(
    cursor: &mut EventCursor<'_>,
    output_index: &Value,
    item_id: &Value,
    object: &Map<String, Value>,
) {
    let mut added = object.clone();
    added.insert(
        "status".to_string(),
        Value::String("in_progress".to_string()),
    );
    added.insert("content".to_string(), Value::Array(Vec::new()));
    cursor.emit(
        "response.output_item.added",
        Map::from_iter([
            (String::from("output_index"), output_index.clone()),
            (String::from("item"), Value::Object(added)),
        ]),
    );
    let Some(content) = object.get("content").and_then(Value::as_array) else {
        return;
    };
    for (content_index, part) in content.iter().enumerate() {
        let Some(part_object) = part.as_object() else {
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
            part,
            part_object,
        );
    }
}

fn emit_text_part(
    cursor: &mut EventCursor<'_>,
    output_index: &Value,
    item_id: &Value,
    content_index: usize,
    part: &Value,
    part_object: &Map<String, Value>,
) {
    let text = part_object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content_index = Value::Number((content_index as u64).into());
    let mut empty_part = part_object.clone();
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
            (String::from("part"), part.clone()),
        ]),
    );
}

fn emit_call(
    cursor: &mut EventCursor<'_>,
    output_index: &Value,
    item_id: &Value,
    object: &Map<String, Value>,
    item_type: Option<&str>,
) {
    let mut added = object.clone();
    let field = match item_type {
        Some("function_call") => Some("arguments"),
        Some("custom_tool_call") => Some("input"),
        _ => None,
    };
    if let Some(field) = field {
        added.insert(field.to_string(), Value::String(String::new()));
    }
    cursor.emit(
        "response.output_item.added",
        Map::from_iter([
            (String::from("output_index"), output_index.clone()),
            (String::from("item"), Value::Object(added)),
        ]),
    );
    if let Some(field) = field {
        if let Some(text) = object.get(field).and_then(Value::as_str) {
            if !text.is_empty() {
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
                        (String::from("delta"), Value::String(text.to_string())),
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
                    (String::from(field), Value::String(text.to_string())),
                ]),
            );
        }
    }
}

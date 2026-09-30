//! Gemini partial tool-argument patches applied to one JSON value.

use super::{AdapterError, AdapterResult};
use serde_json::{json, Map, Value};

/// Gemini's Vertex streaming contract can split function arguments into
/// `partialArgs` patches. The regular API returns `args`, but accepting the
/// patch form here keeps the non-stream and stream bridges on one contract.
pub(in crate::protocol::adapter) fn function_call_args(
    call: &Map<String, Value>,
) -> AdapterResult<Value> {
    let mut args = call.get("args").cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err(AdapterError::invalid_request());
    }
    if let Some(partial_args) = call.get("partialArgs") {
        apply_partial_args(&mut args, partial_args)?;
    }
    Ok(args)
}

pub(in crate::protocol::adapter) fn apply_partial_args(
    target: &mut Value,
    partial_args: &Value,
) -> AdapterResult<()> {
    let patches = partial_args
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?;
    for patch in patches {
        let Some(patch) = patch.as_object() else {
            return Err(AdapterError::invalid_request());
        };
        let Some(path) = patch.get("jsonPath").and_then(Value::as_str) else {
            return Err(AdapterError::invalid_request());
        };
        let Some(value) = partial_arg_value(patch) else {
            continue;
        };
        // Vertex emits an empty string patch after a value while it is still
        // assembling the argument. Do not erase the last non-empty value.
        if value.as_str().is_some_and(str::is_empty) {
            continue;
        }
        set_json_path(target, path, value)?;
    }
    Ok(())
}

fn partial_arg_value(patch: &Map<String, Value>) -> Option<Value> {
    for key in [
        "stringValue",
        "numberValue",
        "boolValue",
        "booleanValue",
        "nullValue",
        "jsonValue",
        "value",
    ] {
        if let Some(value) = patch.get(key) {
            if key == "jsonValue" {
                if let Some(text) = value.as_str() {
                    return serde_json::from_str(text).ok();
                }
            }
            return Some(value.clone());
        }
    }
    None
}

#[derive(Clone, Debug)]
enum JsonPathSegment {
    Key(String),
    Index(usize),
}

fn set_json_path(target: &mut Value, path: &str, value: Value) -> AdapterResult<()> {
    let segments = parse_json_path(path).ok_or_else(AdapterError::invalid_request)?;
    if segments.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    set_json_path_segments(target, &segments, value)
}

fn parse_json_path(path: &str) -> Option<Vec<JsonPathSegment>> {
    let bytes = path.as_bytes();
    if bytes.first().copied() != Some(b'$') {
        return None;
    }
    let mut index = 1;
    let mut segments = Vec::new();
    while index < bytes.len() {
        match bytes[index] {
            b'.' => {
                index += 1;
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'-'))
                {
                    index += 1;
                }
                if start == index {
                    return None;
                }
                segments.push(JsonPathSegment::Key(path[start..index].to_string()));
            }
            b'[' => {
                index += 1;
                if bytes.get(index).copied() == Some(b'\"') {
                    index += 1;
                    let start = index;
                    while index < bytes.len() && bytes[index] != b'\"' {
                        index += 1;
                    }
                    if index >= bytes.len() {
                        return None;
                    }
                    let key = path[start..index].to_string();
                    index += 1;
                    if bytes.get(index).copied() != Some(b']') {
                        return None;
                    }
                    index += 1;
                    segments.push(JsonPathSegment::Key(key));
                } else {
                    let start = index;
                    while index < bytes.len() && bytes[index].is_ascii_digit() {
                        index += 1;
                    }
                    if start == index || bytes.get(index).copied() != Some(b']') {
                        return None;
                    }
                    let value = path[start..index].parse().ok()?;
                    index += 1;
                    segments.push(JsonPathSegment::Index(value));
                }
            }
            _ => return None,
        }
    }
    Some(segments)
}

fn set_json_path_segments(
    current: &mut Value,
    segments: &[JsonPathSegment],
    value: Value,
) -> AdapterResult<()> {
    let Some(segment) = segments.first() else {
        *current = value;
        return Ok(());
    };
    let last = segments.len() == 1;
    match segment {
        JsonPathSegment::Key(key) => {
            let object = current
                .as_object_mut()
                .ok_or_else(AdapterError::invalid_request)?;
            if last {
                object.insert(key.clone(), value);
                return Ok(());
            }
            let next_is_index = matches!(segments[1], JsonPathSegment::Index(_));
            let child = object.entry(key.clone()).or_insert_with(|| {
                if next_is_index {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                }
            });
            set_json_path_segments(child, &segments[1..], value)
        }
        JsonPathSegment::Index(index) => {
            let array = current
                .as_array_mut()
                .ok_or_else(AdapterError::invalid_request)?;
            if *index >= array.len() {
                array.resize_with(index.saturating_add(1), || Value::Null);
            }
            if last {
                array[*index] = value;
                return Ok(());
            }
            if array[*index].is_null() {
                array[*index] = if matches!(segments[1], JsonPathSegment::Index(_)) {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                };
            }
            set_json_path_segments(&mut array[*index], &segments[1..], value)
        }
    }
}

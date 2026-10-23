//! Gemini partial tool-argument patches applied to one JSON value.

use super::{AdapterError, AdapterResult};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(super) struct GeminiArgumentState {
    paths: BTreeMap<Vec<JsonPathSegment>, bool>,
    snapshot: Option<Value>,
}

impl GeminiArgumentState {
    pub(super) fn has_open_fields(&self) -> bool {
        self.paths.values().any(|open| *open)
    }
}

/// Gemini's Vertex streaming contract can split function arguments into
/// `partialArgs` patches. The regular API returns `args`, but accepting the
/// patch form here keeps the non-stream and stream bridges on one contract.
pub(in crate::protocol::adapter) fn function_call_args(
    call: &Map<String, Value>,
) -> AdapterResult<Value> {
    if call.get("willContinue").is_some_and(|flag| flag != false) {
        return Err(AdapterError::invalid_request());
    }
    let mut state = GeminiArgumentState::default();
    let arguments = merge_function_call_args(&json!({}), call, &mut state)?;
    if state.has_open_fields() {
        return Err(AdapterError::invalid_request());
    }
    Ok(arguments)
}

/// A full snapshot may finish previously streamed arguments, but cannot replace
/// them with a different call. String fragments must remain prefixes.
pub(in crate::protocol::adapter) fn merge_function_call_args(
    previous: &Value,
    call: &Map<String, Value>,
    state: &mut GeminiArgumentState,
) -> AdapterResult<Value> {
    if call
        .get("willContinue")
        .is_some_and(|flag| !flag.is_boolean())
    {
        return Err(AdapterError::invalid_request());
    }
    let mut tool_arguments = match call.get("args") {
        Some(snapshot)
            if snapshot.is_object()
                && extends_arguments(previous, snapshot, &mut Vec::new(), state) =>
        {
            for (path, open) in &mut state.paths {
                let old_value = json_path_value(previous, path);
                let next_value = json_path_value(snapshot, path);
                if next_value.is_none() || (!*open && old_value != next_value) {
                    return Err(AdapterError::invalid_request());
                }
                if call.get("willContinue").and_then(Value::as_bool) != Some(true) {
                    *open = false;
                }
            }
            state.snapshot = Some(snapshot.clone());
            snapshot.clone()
        }
        Some(_) => return Err(AdapterError::invalid_request()),
        None => previous.clone(),
    };
    if let Some(partial_args) = call.get("partialArgs") {
        apply_partial_args(&mut tool_arguments, partial_args, state)?;
    }
    Ok(tool_arguments)
}

fn extends_arguments(
    previous: &Value,
    next: &Value,
    path: &mut Vec<JsonPathSegment>,
    state: &GeminiArgumentState,
) -> bool {
    match (previous, next) {
        (Value::Object(previous), Value::Object(next)) => previous.iter().all(|(key, value)| {
            path.push(JsonPathSegment::Key(key.clone()));
            let valid = next
                .get(key)
                .is_some_and(|next| extends_arguments(value, next, path, state));
            path.pop();
            valid
        }),
        (Value::Array(previous), Value::Array(next)) => {
            previous.len() <= next.len()
                && previous
                    .iter()
                    .zip(next)
                    .enumerate()
                    .all(|(index, (previous, next))| {
                        path.push(JsonPathSegment::Index(index));
                        let valid = extends_arguments(previous, next, path, state);
                        path.pop();
                        valid
                    })
        }
        (Value::String(previous), Value::String(next)) => next.starts_with(previous),
        (Value::Null, _)
            if !state.paths.contains_key(path)
                && state
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| json_path_value(snapshot, path))
                    .is_none() =>
        {
            // Sparse array padding is absent data, unlike an explicit null.
            true
        }
        _ => previous == next,
    }
}

fn apply_partial_args(
    target: &mut Value,
    partial_args: &Value,
    state: &mut GeminiArgumentState,
) -> AdapterResult<()> {
    let patches = partial_args
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?;
    for partial_patch in patches {
        let Some(patch_object) = partial_patch.as_object() else {
            return Err(AdapterError::invalid_request());
        };
        super::super::contracts::validate_bridge_fields(
            partial_patch,
            &[
                "jsonPath",
                "willContinue",
                "stringValue",
                "numberValue",
                "boolValue",
                "booleanValue",
                "nullValue",
                "jsonValue",
                "value",
            ],
        )?;
        let Some(json_path) = patch_object.get("jsonPath").and_then(Value::as_str) else {
            return Err(AdapterError::invalid_request());
        };
        if patch_object
            .get("willContinue")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(AdapterError::invalid_request());
        }
        let path = parse_json_path(json_path)
            .filter(|path| !path.is_empty() && path.len() <= 64)
            .ok_or_else(AdapterError::invalid_request)?;
        let previous_open = state.paths.get(&path).copied();
        if previous_open == Some(false) {
            return Err(AdapterError::invalid_request());
        }
        let open = patch_object.get("willContinue").and_then(Value::as_bool) == Some(true);
        let Some(argument_value) = partial_arg_value(patch_object)? else {
            if previous_open != Some(true) || open {
                return Err(AdapterError::invalid_request());
            }
            state.paths.insert(path, false);
            continue;
        };
        if (open || previous_open == Some(true)) && !argument_value.is_string() {
            return Err(AdapterError::invalid_request());
        }
        if previous_open.is_none()
            && (state
                .paths
                .keys()
                .any(|existing| existing.starts_with(&path) || path.starts_with(existing))
                || state
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| json_path_value(snapshot, &path))
                    .is_some())
        {
            return Err(AdapterError::invalid_request());
        }
        set_json_path_segments(target, &path, argument_value, previous_open == Some(true))?;
        state.paths.insert(path, open);
    }
    Ok(())
}

fn partial_arg_value(patch_object: &Map<String, Value>) -> AdapterResult<Option<Value>> {
    let values = [
        "stringValue",
        "numberValue",
        "boolValue",
        "booleanValue",
        "nullValue",
        "jsonValue",
        "value",
    ]
    .into_iter()
    .filter_map(|key| patch_object.get(key).map(|value| (key, value)))
    .collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(AdapterError::invalid_request());
    }
    if let Some((key, patch_value)) = values.first().copied() {
        let valid = match key {
            "stringValue" => patch_value.is_string(),
            "numberValue" => patch_value.is_number(),
            "boolValue" | "booleanValue" => patch_value.is_boolean(),
            "nullValue" => {
                patch_value.is_null()
                    || patch_value.as_u64() == Some(0)
                    || patch_value.as_str() == Some("NULL_VALUE")
            }
            _ => true,
        };
        if !valid {
            return Err(AdapterError::invalid_request());
        }
        if key == "nullValue" {
            return Ok(Some(Value::Null));
        }
        if key == "jsonValue" {
            if let Some(text) = patch_value.as_str() {
                return serde_json::from_str(text)
                    .map(Some)
                    .map_err(|_| AdapterError::invalid_request());
            }
        }
        return Ok(Some(patch_value.clone()));
    }
    Ok(None)
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum JsonPathSegment {
    Key(String),
    Index(usize),
}

fn json_path_value<'a>(value: &'a Value, path: &[JsonPathSegment]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, segment| match segment {
            JsonPathSegment::Key(key) => current.as_object()?.get(key),
            JsonPathSegment::Index(index) => current.as_array()?.get(*index),
        })
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
                if matches!(bytes.get(index), Some(b'"' | b'\'')) {
                    let key = parse_quoted_key(bytes, &mut index)?;
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
                    let array_index = path[start..index].parse().ok()?;
                    index += 1;
                    segments.push(JsonPathSegment::Index(array_index));
                }
            }
            _ => return None,
        }
    }
    Some(segments)
}

fn parse_quoted_key(bytes: &[u8], index: &mut usize) -> Option<String> {
    let quote = *bytes.get(*index)?;
    let mut encoded = vec![b'"'];
    *index += 1;
    while let Some(byte) = bytes.get(*index).copied() {
        *index += 1;
        match byte {
            byte if byte == quote => {
                encoded.push(b'"');
                return serde_json::from_slice(&encoded).ok();
            }
            b'\\' => {
                let escaped = *bytes.get(*index)?;
                *index += 1;
                if escaped == b'\'' && quote == b'\'' {
                    encoded.push(b'\'');
                } else {
                    encoded.extend_from_slice(&[b'\\', escaped]);
                }
            }
            b'"' => encoded.extend_from_slice(b"\\\""),
            _ => encoded.push(byte),
        }
    }
    None
}

fn set_json_path_segments(
    current_value: &mut Value,
    segments: &[JsonPathSegment],
    replacement_value: Value,
    append_string: bool,
) -> AdapterResult<()> {
    let Some(segment) = segments.first() else {
        if append_string {
            if let (Value::String(current), Some(fragment)) =
                (&mut *current_value, replacement_value.as_str())
            {
                current.push_str(fragment);
                return Ok(());
            }
        }
        *current_value = replacement_value;
        return Ok(());
    };
    match segment {
        JsonPathSegment::Key(key) => {
            let object_fields = current_value
                .as_object_mut()
                .ok_or_else(AdapterError::invalid_request)?;
            let next_is_index = matches!(segments.get(1), Some(JsonPathSegment::Index(_)));
            let child = object_fields.entry(key.clone()).or_insert_with(|| {
                if segments.len() == 1 {
                    Value::Null
                } else if next_is_index {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                }
            });
            set_json_path_segments(child, &segments[1..], replacement_value, append_string)
        }
        JsonPathSegment::Index(index) => {
            let array = current_value
                .as_array_mut()
                .ok_or_else(AdapterError::invalid_request)?;
            // A tiny path must not allocate an unbounded sparse array.
            if *index >= 65_536 {
                return Err(AdapterError::invalid_request());
            }
            if *index >= array.len() {
                array.resize_with(index + 1, || Value::Null);
            }
            if segments.len() > 1 && array[*index].is_null() {
                array[*index] = if matches!(segments[1], JsonPathSegment::Index(_)) {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                };
            }
            set_json_path_segments(
                &mut array[*index],
                &segments[1..],
                replacement_value,
                append_string,
            )
        }
    }
}

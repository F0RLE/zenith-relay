use super::*;

mod bundles;
mod lines;

use bundles::{
    is_portable_bundle, is_zenith_bundle, parse_account_container, parse_portable_bundle,
    parse_zenith_bundle, wrapped_sub2api_payload,
};
use lines::{normalize_token_value, parse_json_lines};

pub(super) struct InputEntry {
    pub(super) ordinal: usize,
    pub(super) value: Option<Value>,
    pub(super) issue: Option<ImportIssue>,
}

pub(super) type ParsedEntries = (
    ImportFormat,
    Vec<InputEntry>,
    Vec<ImportWarning>,
    Option<String>,
);

pub(super) fn parse_entries(input: &str) -> Result<ParsedEntries, ImportError> {
    match serde_json::from_str::<Value>(input) {
        Ok(value) => parse_json_value(value),
        Err(_) => parse_json_lines(input),
    }
}

fn parse_json_value(value: Value) -> Result<ParsedEntries, ImportError> {
    ensure_depth(&value)?;
    if let Some(object) = value.as_object() {
        if is_zenith_bundle(object) {
            return parse_zenith_bundle(object);
        }
        if let Some(payload) = wrapped_sub2api_payload(object) {
            return parse_portable_bundle(payload);
        }
        if is_portable_bundle(object) {
            return parse_portable_bundle(object);
        }
        if object.get("accounts").is_some_and(Value::is_array) {
            return parse_account_container(object);
        }
        return Ok(single_entry(value));
    }
    if let Some(values) = value.as_array() {
        check_item_count(values.len())?;
        let entries = values
            .iter()
            .cloned()
            .enumerate()
            .map(|(ordinal, value)| InputEntry {
                ordinal,
                value: Some(normalize_token_value(value)),
                issue: None,
            })
            .collect();
        return Ok((ImportFormat::JsonArray, entries, Vec::new(), None));
    }
    Ok(single_entry(normalize_token_value(value)))
}

fn single_entry(value: Value) -> ParsedEntries {
    (
        ImportFormat::JsonObject,
        vec![InputEntry {
            ordinal: 0,
            value: Some(value),
            issue: None,
        }],
        Vec::new(),
        None,
    )
}

pub(super) fn malformed_import_value() -> Value {
    serde_json::json!({ IMPORT_ERROR_MARKER: true })
}

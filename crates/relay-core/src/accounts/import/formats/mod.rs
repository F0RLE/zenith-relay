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
    pub(super) import_value: Option<Value>,
    pub(super) issue: Option<ImportIssue>,
}

pub(super) type ParsedEntries = (
    ImportFormat,
    Vec<InputEntry>,
    Vec<ImportWarning>,
    Option<String>,
);

pub(super) fn parse_entries(import_document: &str) -> Result<ParsedEntries, ImportError> {
    match serde_json::from_str::<Value>(import_document) {
        Ok(json_document) => parse_json_document(json_document),
        Err(_) => parse_json_lines(import_document),
    }
}

fn parse_json_document(json_document: Value) -> Result<ParsedEntries, ImportError> {
    ensure_depth(&json_document)?;
    if let Some(object) = json_document.as_object() {
        if is_zenith_bundle(object) {
            return parse_zenith_bundle(object);
        }
        if let Some(wrapped_payload) = wrapped_sub2api_payload(object) {
            return parse_portable_bundle(wrapped_payload);
        }
        if is_portable_bundle(object) {
            return parse_portable_bundle(object);
        }
        if object.get("accounts").is_some_and(Value::is_array) {
            return parse_account_container(object);
        }
        return Ok(single_import_entry(json_document));
    }
    if let Some(array_items) = json_document.as_array() {
        check_item_count(array_items.len())?;
        let parsed_entries = array_items
            .iter()
            .cloned()
            .enumerate()
            .map(|(ordinal, item_value)| InputEntry {
                ordinal,
                import_value: Some(normalize_token_value(item_value)),
                issue: None,
            })
            .collect();
        return Ok((ImportFormat::JsonArray, parsed_entries, Vec::new(), None));
    }
    Ok(single_import_entry(normalize_token_value(json_document)))
}

fn single_import_entry(import_value: Value) -> ParsedEntries {
    (
        ImportFormat::JsonObject,
        vec![InputEntry {
            ordinal: 0,
            import_value: Some(import_value),
            issue: None,
        }],
        Vec::new(),
        None,
    )
}

pub(super) fn malformed_import_value() -> Value {
    serde_json::json!({ IMPORT_ERROR_MARKER: true })
}

use super::*;
use crate::strict_json;

mod bundles;
mod lines;

use bundles::{
    is_portable_bundle, is_zenith_bundle, parse_account_container, parse_portable_bundle,
    parse_zenith_bundle, wrapped_sub2api_payload,
};
use lines::{normalize_token_value, parse_json_lines};

#[derive(Default)]
pub(super) struct InputEntry {
    pub(super) ordinal: usize,
    pub(super) import_value: Option<Value>,
    pub(super) issue: Option<ImportIssue>,
    pub(super) format: Option<ImportFormat>,
    pub(super) proxy: Option<url::Url>,
}

pub(super) type ParsedEntries = (
    ImportFormat,
    Vec<InputEntry>,
    Vec<ImportWarning>,
    Option<String>,
);

pub(super) fn parse_entries(import_document: &str) -> Result<ParsedEntries, ImportError> {
    match strict_json::parse(import_document) {
        Ok(json_document) => parse_json_document(json_document),
        Err(strict_json::ParseError::DuplicateField) => Err(ImportError::new(
            ImportErrorCode::MalformedJson,
            "import JSON contains duplicate fields",
        )),
        Err(_) => parse_json_lines(import_document),
    }
}

fn bundle_entries(value: &Value) -> Option<Result<ParsedEntries, ImportError>> {
    let object = value.as_object()?;
    if is_zenith_bundle(object) {
        Some(parse_zenith_bundle(object))
    } else if let Some(payload) = wrapped_sub2api_payload(object) {
        Some(parse_portable_bundle(payload))
    } else if is_portable_bundle(object) {
        Some(parse_portable_bundle(object))
    } else if object.get("accounts").is_some_and(Value::is_array) {
        Some(parse_account_container(object))
    } else {
        None
    }
}

fn parse_json_document(json_document: Value) -> Result<ParsedEntries, ImportError> {
    ensure_depth(&json_document)?;
    if let Some(bundle) = bundle_entries(&json_document) {
        return bundle;
    }
    if let Value::Array(array_items) = json_document {
        check_item_count(array_items.len())?;
        let mut entries = Vec::with_capacity(array_items.len());
        let mut warnings = Vec::new();
        for value in array_items {
            if let Some(bundle) = bundle_entries(&value) {
                let (format, inner, issues, _) = bundle?;
                check_item_count(entries.len() + inner.len())?;
                for mut entry in inner {
                    entry.ordinal = entries.len();
                    entry.format = Some(format);
                    entries.push(entry);
                }
                warnings.extend(issues);
            } else {
                check_item_count(entries.len() + 1)?;
                entries.push(InputEntry {
                    ordinal: entries.len(),
                    import_value: Some(normalize_token_value(value)),
                    ..Default::default()
                });
            }
        }
        return Ok((ImportFormat::JsonArray, entries, warnings, None));
    }
    Ok(single_import_entry(normalize_token_value(json_document)))
}

fn single_import_entry(import_value: Value) -> ParsedEntries {
    (
        ImportFormat::JsonObject,
        vec![InputEntry {
            ordinal: 0,
            import_value: Some(import_value),
            ..Default::default()
        }],
        Vec::new(),
        None,
    )
}

pub(super) fn malformed_import_value() -> Value {
    serde_json::json!({ IMPORT_ERROR_MARKER: true })
}

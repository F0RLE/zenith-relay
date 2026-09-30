use super::formats::{malformed_import_value, parse_entries};
use super::item::parse_item;
use super::sanitization::{redact_file_name, sha256_hex};
use super::{
    ImportAuthMode, ImportError, ImportErrorCode, ImportFormat, ImportIssue, ImportIssueCode,
    ImportPreview, ImportPreviewRow, ImportPreviewStatus, ImportQuotaStatus, ParsedImport,
    MAX_IMPORT_BYTES, MAX_IMPORT_ITEMS, MAX_JSON_DEPTH,
};
use serde_json::Value;
use std::collections::HashSet;

pub fn parse_import(
    input: &str,
    source_file: Option<&str>,
    existing_identity_keys: &[String],
) -> Result<ParsedImport, ImportError> {
    if input.is_empty() || input.trim().is_empty() {
        return Err(ImportError::new(
            ImportErrorCode::EmptyInput,
            "import content is empty",
        ));
    }
    if input.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::new(
            ImportErrorCode::InputTooLarge,
            "import content exceeds the size limit",
        ));
    }
    let source_file = validate_source_file(source_file)?;
    let (format, entries, warnings, description) = parse_entries(input)?;
    if entries.len() > MAX_IMPORT_ITEMS {
        return Err(ImportError::new(
            ImportErrorCode::TooManyItems,
            "import content exceeds the item limit",
        ));
    }

    let existing = existing_identity_keys
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut rows = Vec::with_capacity(entries.len());
    let mut items = Vec::with_capacity(entries.len());

    for entry in entries {
        match entry.value {
            Some(value) => {
                match parse_item(&value, entry.ordinal, format, source_file.as_deref()) {
                    Ok(mut parsed) => {
                        let identity_key = parsed.item.identity_key.to_ascii_lowercase();
                        if !seen.insert(identity_key.clone()) {
                            parsed.preview.status = ImportPreviewStatus::Invalid;
                            parsed.preview.error = Some(ImportIssue::new(
                                ImportIssueCode::DuplicateItem,
                                "duplicate import item",
                            ));
                            parsed.preview.default_selected = false;
                            parsed.preview.selectable = false;
                            rows.push(parsed.preview);
                            continue;
                        }
                        if existing.contains(&identity_key) {
                            parsed.preview.status = ImportPreviewStatus::Existing;
                            parsed.preview.default_selected = false;
                            parsed.preview.existing = true;
                        }
                        rows.push(parsed.preview);
                        items.push(parsed.item);
                    }
                    Err(issue) => rows.push(invalid_row(
                        entry.ordinal,
                        format,
                        source_file.as_deref(),
                        issue,
                    )),
                }
            }
            None => rows.push(invalid_row(
                entry.ordinal,
                format,
                source_file.as_deref(),
                entry.issue.unwrap_or_else(|| {
                    ImportIssue::new(ImportIssueCode::MalformedJson, "malformed JSON item")
                }),
            )),
        }
    }

    Ok(ParsedImport {
        preview: ImportPreview {
            format,
            description,
            rows,
            warnings,
        },
        items,
    })
}

pub fn combine_import_documents(documents: &[String]) -> Result<String, ImportError> {
    if documents.is_empty() {
        return Err(ImportError::new(
            ImportErrorCode::EmptyInput,
            "import content is empty",
        ));
    }

    let mut total_bytes = 0usize;
    let mut values = Vec::new();
    for document in documents {
        total_bytes = total_bytes.checked_add(document.len()).ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::InputTooLarge,
                "import content exceeds the size limit",
            )
        })?;
        if total_bytes > MAX_IMPORT_BYTES {
            return Err(ImportError::new(
                ImportErrorCode::InputTooLarge,
                "import content exceeds the size limit",
            ));
        }
        if document.trim().is_empty() {
            values.push(malformed_import_value());
            check_item_count(values.len())?;
            continue;
        }

        let entries = match parse_entries(document) {
            Ok((_, entries, _, _)) => entries,
            Err(error)
                if matches!(
                    error.code,
                    ImportErrorCode::EmptyInput | ImportErrorCode::MalformedJson
                ) =>
            {
                values.push(malformed_import_value());
                check_item_count(values.len())?;
                continue;
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            values.push(entry.value.unwrap_or_else(malformed_import_value));
            check_item_count(values.len())?;
        }
    }

    let combined = serde_json::to_string(&values).map_err(|_| {
        ImportError::new(
            ImportErrorCode::MalformedJson,
            "failed to combine import documents",
        )
    })?;
    if combined.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::new(
            ImportErrorCode::InputTooLarge,
            "import content exceeds the size limit",
        ));
    }
    Ok(combined)
}

fn invalid_row(
    ordinal: usize,
    format: ImportFormat,
    source_file: Option<&str>,
    issue: ImportIssue,
) -> ImportPreviewRow {
    let seed = format!(
        "{}:{}:{:?}:{:?}",
        source_file.unwrap_or("pasted"),
        ordinal,
        format,
        issue.code
    );
    ImportPreviewRow {
        item_id: format!("import_{}", &sha256_hex(&seed, None, None)[..16]),
        source_file: source_file.map(redact_file_name),
        label: format!("Item {}", ordinal + 1),
        identity: "unknown".to_string(),
        auth_mode: ImportAuthMode::Unknown,
        source_name: format_name(format).to_string(),
        quota_status: ImportQuotaStatus::Skipped,
        status: ImportPreviewStatus::Invalid,
        plan: None,
        expires_at: None,
        subscription_expires_at: None,
        error: Some(issue),
        default_selected: false,
        selectable: false,
        existing: false,
        warnings: Vec::new(),
    }
}

fn validate_source_file(source_file: Option<&str>) -> Result<Option<String>, ImportError> {
    let Some(source_file) = source_file else {
        return Ok(None);
    };
    let source_file = source_file.trim();
    if source_file.is_empty()
        || source_file.len() > 128
        || source_file == "."
        || source_file == ".."
        || source_file.contains(['/', '\\'])
        || source_file.chars().any(char::is_control)
    {
        return Err(ImportError::new(
            ImportErrorCode::InvalidSourceFile,
            "source file name is unsafe",
        ));
    }
    Ok(Some(source_file.to_string()))
}

pub(in crate::accounts::import) fn ensure_depth(root: &Value) -> Result<(), ImportError> {
    let mut stack = vec![(root, 1usize)];
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_JSON_DEPTH {
            return Err(ImportError::new(
                ImportErrorCode::JsonTooDeep,
                "import JSON exceeds the nesting limit",
            ));
        }
        match value {
            Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                stack.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(in crate::accounts::import) fn check_item_count(count: usize) -> Result<(), ImportError> {
    if count > MAX_IMPORT_ITEMS {
        Err(ImportError::new(
            ImportErrorCode::TooManyItems,
            "import content exceeds the item limit",
        ))
    } else {
        Ok(())
    }
}

fn format_name(value: ImportFormat) -> &'static str {
    match value {
        ImportFormat::JsonObject => "json_object",
        ImportFormat::JsonArray => "json_array",
        ImportFormat::JsonLines => "json_lines",
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
        ImportFormat::ZenithV1 => "zenith",
    }
}

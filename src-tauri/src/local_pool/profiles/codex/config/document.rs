use super::super::*;

pub(in crate::local_pool::profiles::codex) fn parse_config(content: &str) -> Result<DocumentMut> {
    match content.parse::<DocumentMut>() {
        Ok(document) => Ok(document),
        Err(original_error) => {
            // Older Codex builds wrote Windows paths into basic TOML strings
            // without escaping backslashes (for example `C:\\Users`). Repair
            // only path-like strings so a stale config can still be migrated
            // safely; unrelated TOML errors remain fail-closed.
            let repaired = repair_windows_basic_strings(content);
            if repaired == content {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("ChatGPT config is not valid TOML: {original_error}"),
                ));
            }
            repaired.parse::<DocumentMut>().map_err(|error| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("ChatGPT config is not valid TOML: {error}"),
                )
            })
        }
    }
}

fn repair_windows_basic_strings(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut repaired = String::with_capacity(content.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'"' {
            let next = content[cursor..]
                .find('"')
                .map_or(bytes.len(), |offset| cursor + offset);
            repaired.push_str(&content[cursor..next]);
            cursor = next;
            continue;
        }
        let start = cursor;
        cursor += 1;
        let mut end = cursor;
        while end < bytes.len() {
            if bytes[end] == b'"' {
                let mut slashes = 0;
                let mut index = end;
                while index > start + 1 && bytes[index - 1] == b'\\' {
                    slashes += 1;
                    index -= 1;
                }
                if slashes % 2 == 0 {
                    break;
                }
            }
            end += 1;
        }
        if end >= bytes.len() {
            repaired.push_str(&content[start..]);
            break;
        }
        let value = &content[cursor..end];
        repaired.push('"');
        if value.contains(":\\") {
            let mut index = 0;
            while index < value.len() {
                let byte = value.as_bytes()[index];
                if byte != b'\\' {
                    let next = value[index..]
                        .find('\\')
                        .map_or(value.len(), |offset| index + offset);
                    repaired.push_str(&value[index..next]);
                    index = next;
                    continue;
                }
                let run_start = index;
                while index < value.len() && value.as_bytes()[index] == b'\\' {
                    index += 1;
                }
                let run = &value[run_start..index];
                if run.len() == 1 {
                    repaired.push_str("\\\\");
                } else {
                    repaired.push_str(run);
                }
            }
        } else {
            repaired.push_str(value);
        }
        repaired.push('"');
        cursor = end + 1;
    }
    repaired
}

pub(in crate::local_pool::profiles::codex) fn validate_config_shape(
    document: &DocumentMut,
) -> Result<()> {
    if document.get("model_provider").is_some()
        && document
            .get("model_provider")
            .and_then(Item::as_str)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_provider must be a string",
        ));
    }
    if document.get("model_providers").is_some()
        && document
            .get("model_providers")
            .and_then(Item::as_table)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_providers must be a table",
        ));
    }
    if document.get("model_reasoning_effort").is_some()
        && document
            .get("model_reasoning_effort")
            .and_then(Item::as_str)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_reasoning_effort must be a string",
        ));
    }
    Ok(())
}

pub(in crate::local_pool::profiles::codex) fn desktop_bool(
    document: &DocumentMut,
    key: &str,
) -> Option<bool> {
    document
        .get("desktop")
        .and_then(Item::as_table_like)
        .and_then(|desktop| desktop.get(key))
        .and_then(Item::as_bool)
}

pub(in crate::local_pool::profiles::codex) fn root_model_provider(
    document: &DocumentMut,
) -> Option<String> {
    document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(in crate::local_pool::profiles::codex) fn root_model_catalog_json(
    document: &DocumentMut,
) -> Option<String> {
    document
        .get("model_catalog_json")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(in crate::local_pool::profiles::codex) fn root_model_reasoning_effort(
    document: &DocumentMut,
) -> Option<String> {
    document
        .get("model_reasoning_effort")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(in crate::local_pool::profiles::codex) fn root_openai_base_url(
    document: &DocumentMut,
) -> Option<String> {
    document
        .get("openai_base_url")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(in crate::local_pool::profiles::codex) fn document_has_provider(
    document: &DocumentMut,
) -> bool {
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| providers.contains_key(PROVIDER_ID))
}

pub(in crate::local_pool::profiles::codex) fn key_hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub(in crate::local_pool::profiles::codex) fn bytes_hash(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

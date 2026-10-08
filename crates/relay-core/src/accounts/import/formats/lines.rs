use super::super::*;
use super::{InputEntry, ParsedEntries};

pub(super) fn parse_json_lines(import_document: &str) -> Result<ParsedEntries, ImportError> {
    let non_empty_lines = import_document
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if non_empty_lines.is_empty() {
        return Err(ImportError::new(
            ImportErrorCode::EmptyInput,
            "import content is empty",
        ));
    }
    check_item_count(non_empty_lines.len())?;

    let has_multiple_lines = non_empty_lines.len() > 1;
    let mut parsed_entries = Vec::with_capacity(non_empty_lines.len());
    for (ordinal, line) in non_empty_lines.into_iter().enumerate() {
        match serde_json::from_str::<Value>(line) {
            Ok(json_value) => {
                ensure_depth(&json_value)?;
                parsed_entries.push(InputEntry {
                    ordinal,
                    import_value: Some(normalize_token_value(json_value)),
                    issue: None,
                });
            }
            Err(_) => match raw_access_token(line) {
                Some(token) => parsed_entries.push(InputEntry {
                    ordinal,
                    import_value: Some(access_token_value(token)),
                    issue: None,
                }),
                None if has_multiple_lines => parsed_entries.push(InputEntry {
                    ordinal,
                    import_value: None,
                    issue: Some(ImportIssue::new(
                        ImportIssueCode::MalformedJson,
                        "malformed JSON or access token line",
                    )),
                }),
                None => {
                    return Err(ImportError::new(
                        ImportErrorCode::MalformedJson,
                        "import content is not valid JSON or an access token",
                    ));
                }
            },
        }
    }
    Ok((ImportFormat::JsonLines, parsed_entries, Vec::new(), None))
}

pub(super) fn normalize_token_value(token_value: Value) -> Value {
    match token_value {
        Value::String(token_text) => raw_access_token(token_text.as_str())
            .map(access_token_value)
            .unwrap_or(Value::String(token_text)),
        other_value => other_value,
    }
}

fn raw_access_token(token_text: &str) -> Option<&str> {
    let trimmed_token = token_text.trim();
    let token = trimmed_token
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .and_then(|_| trimmed_token.get(7..))
        .map(str::trim)
        .unwrap_or(trimmed_token);
    if token.is_empty()
        || token.len() > MAX_RAW_TOKEN_BYTES
        || !token.is_ascii()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return None;
    }
    let mut parts = token.split('.');
    let jwt = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(header), Some(payload), Some(signature), None)
            if !header.is_empty() && !payload.is_empty() && !signature.is_empty()
    );
    (jwt || token
        .strip_prefix("at-")
        .is_some_and(|suffix| !suffix.is_empty()))
    .then_some(token)
}

fn access_token_value(token: &str) -> Value {
    serde_json::json!({ "access_token": token })
}

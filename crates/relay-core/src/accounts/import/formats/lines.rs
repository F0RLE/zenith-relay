use super::super::*;
use super::{InputEntry, ParsedEntries};

pub(super) fn parse_json_lines(input: &str) -> Result<ParsedEntries, ImportError> {
    let lines = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Err(ImportError::new(
            ImportErrorCode::EmptyInput,
            "import content is empty",
        ));
    }
    check_item_count(lines.len())?;

    let multiple = lines.len() > 1;
    let mut entries = Vec::with_capacity(lines.len());
    for (ordinal, line) in lines.into_iter().enumerate() {
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                ensure_depth(&value)?;
                entries.push(InputEntry {
                    ordinal,
                    value: Some(normalize_token_value(value)),
                    issue: None,
                });
            }
            Err(_) => match raw_access_token(line) {
                Some(token) => entries.push(InputEntry {
                    ordinal,
                    value: Some(access_token_value(token)),
                    issue: None,
                }),
                None if multiple => entries.push(InputEntry {
                    ordinal,
                    value: None,
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
    Ok((ImportFormat::JsonLines, entries, Vec::new(), None))
}

pub(super) fn normalize_token_value(value: Value) -> Value {
    match value {
        Value::String(value) => raw_access_token(&value)
            .map(access_token_value)
            .unwrap_or(Value::String(value)),
        value => value,
    }
}

fn raw_access_token(value: &str) -> Option<&str> {
    let value = value.trim();
    let token = value
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .and_then(|_| value.get(7..))
        .map(str::trim)
        .unwrap_or(value);
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
        .is_some_and(|value| !value.is_empty()))
    .then_some(token)
}

fn access_token_value(token: &str) -> Value {
    serde_json::json!({ "access_token": token })
}

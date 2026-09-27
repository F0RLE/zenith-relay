//! Redaction for diagnostic text.
//!
//! Logs may keep operation names and error codes. Credentials, account-like
//! tokens, emails, and raw free-form text do not.

use super::{MAX_TEXT_BYTES, REDACTED};

pub(super) fn safe_detail(value: &str) -> String {
    safe_text(value, MAX_TEXT_BYTES)
}

/// Preserve only the small, canonical alphabet used by diagnostic/error
/// codes.  Running codes through `safe_text` would treat prefixes such as
/// `source_` as identity-like text and turn useful labels into
/// `source_[redacted]`.
pub(super) fn safe_code(value: &str) -> String {
    let value = value.trim();
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        value.to_ascii_lowercase()
    } else {
        REDACTED.to_string()
    }
}

pub(super) fn safe_text(value: &str, max_bytes: usize) -> String {
    let text = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = redact_secrets(&text);
    truncate_text(&redacted, max_bytes)
}

pub(super) fn truncate_text(value: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let ellipsis = "…";
    let mut end = max_bytes.saturating_sub(ellipsis.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], ellipsis)
}

fn redact_secrets(value: &str) -> String {
    // Bearer credentials must be removed before generic `authorization: ...`
    // handling; otherwise an unquoted header could leave the token after the
    // first whitespace delimiter.
    let mut output = redact_bearer_values(value.to_string());
    output = redact_url_credentials(output);
    output = redact_sensitive_query_values(output);
    output = redact_sensitive_keys(output);
    output = redact_email_like(&output);
    output = redact_identity_like(&output);
    output = redact_uuid_like_substrings(&output);
    let mut result = String::with_capacity(output.len());
    let mut token = String::new();
    for character in output.chars() {
        if character.is_ascii_alphanumeric() || "._-".contains(character) {
            token.push(character);
            continue;
        }
        append_redacted_token(&mut result, &token);
        token.clear();
        result.push(character);
    }
    append_redacted_token(&mut result, &token);
    result
}

fn redact_sensitive_keys(mut output: String) -> String {
    for key in [
        "token",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
        "id_token",
        "idtoken",
        "api_key",
        "api-key",
        "apikey",
        "x_api_key",
        "x-api-key",
        "xapikey",
        "authorization",
        "password",
        "cookie",
        "set-cookie",
        "setcookie",
        "session_id",
        "sessionid",
        "client_secret",
        "clientsecret",
        "account_id",
        "accountid",
        "provider_account_id",
        "provideraccountid",
        "user_id",
        "userid",
        "identity",
        "email",
        "secret",
        "private_key",
        "privatekey",
        "prompt",
        "prompts",
        "input",
        "inputs",
        "output",
        "outputs",
        "content",
        "body",
        "request",
        "response",
        "headers",
        "messages",
        "tools",
        "arguments",
    ] {
        let mut search_from = 0;
        loop {
            let lower = output.to_ascii_lowercase();
            let Some(relative) = lower[search_from..].find(key) else {
                break;
            };
            let key_start = search_from + relative;
            let key_end = key_start + key.len();
            let boundary_before = key_start == 0
                || !lower[..key_start]
                    .chars()
                    .next_back()
                    .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
            if !boundary_before {
                search_from = key_end;
                continue;
            }
            let mut cursor = skip_key_separator(&output, key_end);
            if cursor >= output.len()
                || !matches!(output[cursor..].chars().next(), Some(':') | Some('='))
            {
                search_from = key_end;
                continue;
            }
            cursor += 1;
            let Some((start, end)) = sensitive_value_range(&output, cursor) else {
                search_from = key_end;
                continue;
            };
            if start < end {
                output.replace_range(start..end, "[redacted]");
                search_from = start + REDACTED.len();
            } else {
                search_from = key_end;
            }
            if search_from >= output.len() {
                break;
            }
        }
    }
    output
}

fn skip_key_separator(value: &str, mut cursor: usize) -> usize {
    while cursor < value.len() {
        let Some(character) = value[cursor..].chars().next() else {
            break;
        };
        if !(character.is_ascii_whitespace() || character == '"' || character == '\'') {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}

/// Locate one sensitive value without treating an escaped quote as its end.
/// Malformed quoted input is redacted through the end of the diagnostic, which
/// is safer than attempting to preserve a possibly secret suffix.
fn sensitive_value_range(value: &str, mut cursor: usize) -> Option<(usize, usize)> {
    while cursor < value.len()
        && value[cursor..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_whitespace())
    {
        cursor += value[cursor..].chars().next()?.len_utf8();
    }
    if cursor >= value.len() {
        return Some((cursor, cursor));
    }
    let first = value[cursor..].chars().next()?;
    if first == '"' || first == '\'' {
        let quote = first;
        let start = cursor + quote.len_utf8();
        let mut escaped = false;
        for (offset, character) in value[start..].char_indices() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                return Some((start, start + offset));
            }
        }
        return Some((start, value.len()));
    }
    let end = value[cursor..]
        .find(|character: char| character.is_ascii_whitespace() || ",;)}\"'&#[".contains(character))
        .map(|offset| cursor + offset)
        .unwrap_or(value.len());
    Some((cursor, end))
}

fn redact_bearer_values(mut output: String) -> String {
    let mut search_from = 0;
    loop {
        let lower = output.to_ascii_lowercase();
        let Some(relative) = lower[search_from..].find("bearer ") else {
            break;
        };
        let start = search_from + relative + "bearer ".len();
        let end = output[start..]
            .find(|character: char| character.is_whitespace() || ",;)}\"'".contains(character))
            .map(|offset| start + offset)
            .unwrap_or(output.len());
        output.replace_range(start..end, REDACTED);
        search_from = start + REDACTED.len();
        if search_from >= output.len() {
            break;
        }
    }
    output
}

fn redact_url_credentials(mut output: String) -> String {
    let mut search_from = 0;
    while let Some(relative) = output[search_from..].find("://") {
        let scheme_end = search_from + relative + 3;
        let authority_end = output[scheme_end..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '/' | '?' | '#')
            })
            .map(|offset| scheme_end + offset)
            .unwrap_or(output.len());
        let Some(at_offset) = output[scheme_end..authority_end].find('@') else {
            search_from = authority_end;
            if search_from >= output.len() {
                break;
            }
            continue;
        };
        let at = scheme_end + at_offset;
        output.replace_range(scheme_end..at, REDACTED);
        search_from = scheme_end + REDACTED.len() + 1;
        if search_from >= output.len() {
            break;
        }
    }
    output
}

fn redact_sensitive_query_values(mut output: String) -> String {
    // Parse one query segment at a time.  Looking for the first `=` after a
    // `?`/`&` marker is subtly wrong for inputs such as `?flag&token=secret`:
    // the key would be seen as `flag&token` and the credential would survive.
    // Collect ranges against the original string and replace from the end so
    // multiple credentials can be removed without invalidating offsets.
    const KEYS: [&str; 22] = [
        "token",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
        "id_token",
        "idtoken",
        "api_key",
        "apikey",
        "secret",
        "code",
        "auth",
        "authorization",
        "session",
        "session_id",
        "sessionid",
        "account_id",
        "accountid",
        "client_secret",
        "clientsecret",
        "x_api_key",
        "xapikey",
    ];
    let mut ranges = Vec::<(usize, usize)>::new();
    let bytes = output.as_bytes();
    let mut marker = 0;
    while marker < bytes.len() {
        if !matches!(bytes[marker], b'?' | b'&') {
            marker += 1;
            continue;
        }
        let key_start = marker + 1;
        let segment_end = output[key_start..]
            .find(|character: char| {
                character.is_ascii_whitespace() || matches!(character, '&' | '#')
            })
            .map(|offset| key_start + offset)
            .unwrap_or(output.len());
        let Some(equal_offset) = output[key_start..segment_end].find('=') else {
            marker = segment_end;
            continue;
        };
        let equal = key_start + equal_offset;
        let key = output[key_start..equal].trim_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, '"' | '\'')
        });
        let normalized_key = key
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if !KEYS.iter().any(|candidate| {
            candidate
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .eq(normalized_key.chars())
        }) {
            marker = segment_end;
            continue;
        }
        let value_start = equal + 1;
        let first_value_character = output[value_start..segment_end].chars().next();
        let value_end =
            if first_value_character.is_some_and(|character| matches!(character, '"' | '\'')) {
                // The quote itself is retained; only its contents are replaced.
                let quote = first_value_character.unwrap();
                let content_start = value_start + quote.len_utf8();
                output[content_start..segment_end]
                    .find(quote)
                    .map(|offset| content_start + offset)
                    .unwrap_or(segment_end)
            } else {
                output[value_start..segment_end]
                    .find(['"', '\''])
                    .map(|offset| value_start + offset)
                    .unwrap_or(segment_end)
            };
        if value_start < value_end {
            let content_start = if output[value_start..value_end]
                .chars()
                .next()
                .is_some_and(|character| matches!(character, '"' | '\''))
            {
                value_start + output[value_start..].chars().next().unwrap().len_utf8()
            } else {
                value_start
            };
            if content_start < value_end {
                ranges.push((content_start, value_end));
            }
        }
        marker = segment_end;
    }
    ranges.sort_unstable_by_key(|range| std::cmp::Reverse(range.0));
    for (start, end) in ranges {
        output.replace_range(start..end, REDACTED);
    }
    output
}

fn redact_identity_like(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut token = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || "._-".contains(character) {
            token.push(character);
            continue;
        }
        append_redacted_identity(&mut result, &token);
        token.clear();
        result.push(character);
    }
    append_redacted_identity(&mut result, &token);
    result
}

/// Remove UUIDs even when they are embedded in a useful operation label such
/// as `delete-account-<uuid>`. Account/source/session identifiers are commonly
/// UUIDs, and keeping the surrounding label preserves enough context to debug
/// the failed action without writing the identifier itself.
fn redact_uuid_like_substrings(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut result = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if is_uuid_at(bytes, cursor)
            && (cursor == 0 || !is_uuid_boundary_byte(bytes[cursor - 1]))
            && (cursor + 36 == bytes.len() || !is_uuid_boundary_byte(bytes[cursor + 36]))
        {
            result.push_str(REDACTED);
            cursor += 36;
        } else {
            let Some(character) = value[cursor..].chars().next() else {
                break;
            };
            result.push(character);
            cursor += character.len_utf8();
        }
    }
    result
}

fn is_uuid_at(bytes: &[u8], start: usize) -> bool {
    if start.saturating_add(36) > bytes.len() {
        return false;
    }
    for (offset, byte) in bytes[start..start + 36].iter().copied().enumerate() {
        if matches!(offset, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn is_uuid_boundary_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

fn append_redacted_identity(output: &mut String, token: &str) {
    let lower = token.to_ascii_lowercase();
    let identity_prefixes = [
        "account_",
        "account-",
        "acct_",
        "acct-",
        "user_",
        "user-",
        "provider_",
        "provider-",
        "source_",
        "source-",
        "session_",
        "session-",
        "import_",
        "import-",
        "proxy_",
        "proxy-",
        "task_",
        "task-",
    ];
    for prefix in identity_prefixes {
        let Some(index) = lower.find(prefix) else {
            continue;
        };
        if index > 0
            && lower[..index]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            continue;
        }
        let suffix = &token[index + prefix.len()..];
        // Keep human-readable operation names such as `account-import` while
        // still hiding account/user identifiers, which normally contain a
        // digit or are long opaque values.
        if suffix.is_empty()
            || (!suffix.chars().any(|character| character.is_ascii_digit()) && suffix.len() < 16)
        {
            continue;
        }
        output.push_str(&token[..index + prefix.len()]);
        output.push_str(REDACTED);
        return;
    }
    output.push_str(token);
}

fn redact_email_like(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut candidate = String::new();
    let flush = |result: &mut String, candidate: &mut String| {
        let at = candidate.find('@');
        let dot_after_at = at.and_then(|index| candidate[index + 1..].find('.'));
        // A URL whose userinfo was already replaced leaves `@host`; retain
        // the host so the diagnostic still identifies the failing endpoint.
        if at.is_some_and(|index| index > 0) && dot_after_at.is_some() {
            result.push_str("[redacted]");
        } else {
            result.push_str(candidate);
        }
        candidate.clear();
    };
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || "._%+-@".contains(character) {
            candidate.push(character);
        } else {
            flush(&mut result, &mut candidate);
            result.push(character);
        }
    }
    flush(&mut result, &mut candidate);
    result
}

fn append_redacted_token(output: &mut String, token: &str) {
    let token_lower = token.to_ascii_lowercase();
    let sensitive_prefix = [
        "sk-",
        "pk-",
        "rk-",
        "znt-",
        "zrs-",
        "ghp_",
        "github_pat_",
        "at-",
        "xoxb-",
        "xoxp-",
    ]
    .iter()
    .any(|prefix| token_lower.starts_with(prefix) && token.len() >= prefix.len() + 8);
    let jwt = token_lower.starts_with("eyj") && token.len() > 24;
    let looks_like_email = token.contains('@') && token.contains('.');
    if sensitive_prefix || jwt || looks_like_email {
        output.push_str(REDACTED);
    } else {
        output.push_str(token);
    }
}

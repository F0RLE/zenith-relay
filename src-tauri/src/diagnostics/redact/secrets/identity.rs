use super::REDACTED;

pub(super) fn redact_identity_like(value: &str) -> String {
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
pub(super) fn redact_uuid_like_substrings(value: &str) -> String {
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

pub(super) fn redact_email_like(value: &str) -> String {
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

pub(super) fn append_redacted_token(output: &mut String, token: &str) {
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

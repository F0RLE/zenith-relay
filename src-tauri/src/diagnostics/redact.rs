//! Redaction for diagnostic text.
//!
//! Logs may keep operation names and error codes. Credentials, account-like
//! tokens, emails, and raw free-form text do not.

use super::{MAX_TEXT_BYTES, REDACTED};

mod secrets;

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
    let redacted = secrets::redact_secrets(&text);
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

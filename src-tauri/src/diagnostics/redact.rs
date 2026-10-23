//! Redaction for diagnostic text.
//!
//! Logs may keep operation names and error codes. Credentials, account-like
//! tokens, emails, and raw free-form text do not.

use super::{MAX_TEXT_BYTES, REDACTED};

mod secrets;

pub(super) fn safe_detail(detail_text: &str) -> String {
    safe_text(detail_text, MAX_TEXT_BYTES)
}

/// Preserve only the small, canonical alphabet used by diagnostic/error
/// codes.  Running codes through `safe_text` would treat prefixes such as
/// `source_` as identity-like text and turn useful labels into
/// `source_[redacted]`.
pub(super) fn safe_code(code_text: &str) -> String {
    let normalized_code = code_text.trim();
    if !normalized_code.is_empty()
        && normalized_code.len() <= 64
        && normalized_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        normalized_code.to_ascii_lowercase()
    } else {
        REDACTED.to_string()
    }
}

pub(super) fn safe_text(raw_text: &str, max_bytes: usize) -> String {
    let normalized_text = raw_text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let normalized_text = normalized_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let redacted = secrets::redact_secrets(&normalized_text);
    truncate_text(&redacted, max_bytes)
}

pub(super) fn truncate_text(text: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let ellipsis = "…";
    let mut end = max_bytes.saturating_sub(ellipsis.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &text[..end], ellipsis)
}

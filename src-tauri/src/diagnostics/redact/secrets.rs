use super::super::REDACTED;

mod identity;
mod keys;
mod urls;

use identity::{
    append_redacted_token, redact_email_like, redact_identity_like, redact_uuid_like_substrings,
};
use keys::redact_sensitive_keys;
use urls::{redact_bearer_values, redact_sensitive_query_values, redact_url_credentials};

pub(super) fn redact_secrets(raw_text: &str) -> String {
    // Bearer credentials must be removed before generic `authorization: ...`
    // handling; otherwise an unquoted header could leave the token after the
    // first whitespace delimiter.
    let mut redacted_text = redact_bearer_values(raw_text.to_string());
    redacted_text = redact_url_credentials(redacted_text);
    redacted_text = redact_sensitive_query_values(redacted_text);
    redacted_text = redact_sensitive_keys(redacted_text);
    redacted_text = redact_email_like(&redacted_text);
    redacted_text = redact_identity_like(&redacted_text);
    redacted_text = redact_uuid_like_substrings(&redacted_text);
    let mut sanitized_text = String::with_capacity(redacted_text.len());
    let mut token = String::new();
    for character in redacted_text.chars() {
        if character.is_ascii_alphanumeric() || "._-".contains(character) {
            token.push(character);
            continue;
        }
        append_redacted_token(&mut sanitized_text, &token);
        token.clear();
        sanitized_text.push(character);
    }
    append_redacted_token(&mut sanitized_text, &token);
    sanitized_text
}

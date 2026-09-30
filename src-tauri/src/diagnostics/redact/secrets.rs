use super::super::REDACTED;

mod identity;
mod keys;
mod urls;

use identity::{
    append_redacted_token, redact_email_like, redact_identity_like, redact_uuid_like_substrings,
};
use keys::redact_sensitive_keys;
use urls::{redact_bearer_values, redact_sensitive_query_values, redact_url_credentials};

pub(super) fn redact_secrets(value: &str) -> String {
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

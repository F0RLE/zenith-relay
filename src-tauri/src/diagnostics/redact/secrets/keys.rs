use super::REDACTED;

pub(super) fn redact_sensitive_keys(mut diagnostic_text: String) -> String {
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
        "2fa",
        "totp",
        "totp_secret",
        "totpsecret",
        "otp_secret",
        "otpsecret",
        "phone",
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
            let lower = diagnostic_text.to_ascii_lowercase();
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
            let mut cursor = skip_key_separator(&diagnostic_text, key_end);
            if cursor >= diagnostic_text.len()
                || !matches!(
                    diagnostic_text[cursor..].chars().next(),
                    Some(':') | Some('=')
                )
            {
                search_from = key_end;
                continue;
            }
            cursor += 1;
            let Some((start, end)) = sensitive_value_range(&diagnostic_text, cursor) else {
                search_from = key_end;
                continue;
            };
            if start < end {
                diagnostic_text.replace_range(start..end, "[redacted]");
                search_from = start + REDACTED.len();
            } else {
                search_from = key_end;
            }
            if search_from >= diagnostic_text.len() {
                break;
            }
        }
    }
    diagnostic_text
}

fn skip_key_separator(diagnostic_text: &str, mut cursor: usize) -> usize {
    while cursor < diagnostic_text.len() {
        let Some(character) = diagnostic_text[cursor..].chars().next() else {
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
fn sensitive_value_range(diagnostic_text: &str, mut cursor: usize) -> Option<(usize, usize)> {
    while cursor < diagnostic_text.len()
        && diagnostic_text[cursor..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_whitespace())
    {
        cursor += diagnostic_text[cursor..].chars().next()?.len_utf8();
    }
    if cursor >= diagnostic_text.len() {
        return Some((cursor, cursor));
    }
    let first = diagnostic_text[cursor..].chars().next()?;
    if first == '"' || first == '\'' {
        let quote = first;
        let start = cursor + quote.len_utf8();
        let mut escaped = false;
        for (offset, character) in diagnostic_text[start..].char_indices() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                return Some((start, start + offset));
            }
        }
        return Some((start, diagnostic_text.len()));
    }
    let end = diagnostic_text[cursor..]
        .find(|character: char| character.is_ascii_whitespace() || ",;)}\"'&#[".contains(character))
        .map(|offset| cursor + offset)
        .unwrap_or(diagnostic_text.len());
    Some((cursor, end))
}

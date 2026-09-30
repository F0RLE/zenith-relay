use super::REDACTED;

pub(super) fn redact_bearer_values(mut output: String) -> String {
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

pub(super) fn redact_url_credentials(mut output: String) -> String {
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

pub(super) fn redact_sensitive_query_values(mut output: String) -> String {
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

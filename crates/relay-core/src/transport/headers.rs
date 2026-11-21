use reqwest::header::{HeaderMap, HeaderValue};
use std::time::SystemTime;

/// Connection options name incoming-hop fields, including otherwise permitted
/// extensions. Inspect bytes so an unrelated opaque token cannot hide them.
pub(crate) fn is_connection_header(headers: &HeaderMap, name: &str) -> bool {
    headers.get_all("connection").iter().any(|value| {
        value
            .as_bytes()
            .split(|byte| *byte == b',')
            .any(|option| option.trim_ascii().eq_ignore_ascii_case(name.as_bytes()))
    })
}

/// Normalized credential field names are excluded in both forwarding directions.
pub(crate) fn is_credential_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-auth-token"
            | "x-api-token"
    ) || name.ends_with("-api-key")
}

/// Read a singleton without arbitrarily choosing among repeated field lines.
pub(crate) fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

/// A relative floor measured at receipt. Consumers must preserve the floor
/// when converting it to a monotonic deadline; do not cap a long provider hint
/// to a shorter polling interval. Repeated singleton fields are not valid wire
/// metadata, but local recovery conservatively honors the longest valid value.
pub(crate) fn retry_after_ms(headers: &HeaderMap, now: SystemTime) -> Option<u64> {
    headers
        .get_all("retry-after")
        .iter()
        .filter_map(|value| retry_after_value_ms(value, now))
        .max()
}

fn retry_after_value_ms(value: &HeaderValue, now: SystemTime) -> Option<u64> {
    let value = value.to_str().ok()?.trim_matches([' ', '\t']);
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = value.bytes().fold(0_u64, |seconds, digit| {
            seconds
                .saturating_mul(10)
                .saturating_add(u64::from(digit - b'0'))
        });
        return Some(seconds.saturating_mul(1_000));
    }
    // A complete HTTP date contains a comma; Retry-After is not a comma-list.
    Some(
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .ok()?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    )
}

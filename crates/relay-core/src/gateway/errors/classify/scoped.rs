//! Explicit edge/transport evidence does not invalidate provider credentials.

use super::*;
use serde::Deserialize;

pub(super) fn forbidden_category(status: StatusCode, payload: &Value) -> Option<&'static str> {
    if status != StatusCode::FORBIDDEN {
        return None;
    }
    if cloudflare_block(payload, false, 5) {
        return Some(error_codes::UPSTREAM_EDGE_CHALLENGE);
    }
    for path in ["/error", "/response/error", "/body/error"] {
        let Some(error) = payload.pointer(path) else {
            continue;
        };
        let field = |name| {
            error
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
        };
        if field("type").eq_ignore_ascii_case("upstream_error")
            && (field("code").is_empty() || field("code").eq_ignore_ascii_case("upstream_error"))
            && field("message").eq_ignore_ascii_case(
                "failed to WebSocket dial: expected handshake response status code 101 but got 403",
            )
        {
            return Some(error_codes::UPSTREAM_WEBSOCKET_HANDSHAKE_REJECTED);
        }
    }
    None
}

fn cloudflare_block(value: &Value, inherited_marker: bool, depth: u8) -> bool {
    if depth == 0 {
        return false;
    }
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .take(16)
            .any(|item| cloudflare_block(item, inherited_marker, depth - 1));
    }
    let Some(fields) = value.as_object() else {
        return false;
    };
    let marked = inherited_marker || fields.get("cloudflare_error") == Some(&Value::Bool(true));
    if fields
        .get("error_name")
        .and_then(Value::as_str)
        .is_some_and(|name| name.trim().eq_ignore_ascii_case("browser_signature_banned"))
        || (marked
            && ["code", "error_code"].iter().any(|key| {
                fields.get(*key).is_some_and(|code| {
                    code.as_u64() == Some(1010)
                        || code.as_str().is_some_and(|text| text.trim() == "1010")
                })
            }))
    {
        return true;
    }
    // Error wrappers only: never recursively classify echoed prompts, client
    // input, request metadata or tool results as security evidence.
    [
        "error",
        "errors",
        "detail",
        "body",
        "response",
        "upstream_error",
    ]
    .iter()
    .filter_map(|key| fields.get(*key))
    .any(|child| cloudflare_block(child, marked, depth - 1))
}

/// A few compatible relays append SSE to an already complete JSON error.
/// Only its first error envelope is authoritative; a later event cannot turn
/// a content refusal into an account failure (or the other way around).
pub(super) fn error_before_sse(body: &[u8]) -> Option<Value> {
    let mut values = serde_json::Deserializer::from_slice(body).into_iter::<Value>();
    let first = values.next()?.ok()?;
    let tail = std::str::from_utf8(&body[values.byte_offset()..])
        .ok()?
        .trim_start();
    if !first.get("error").is_some_and(Value::is_object)
        || !(tail.starts_with("data:") || tail.starts_with("event:"))
    {
        return None;
    }
    // Typed validation rejects duplicate discriminators, malformed fields and
    // unknown envelopes before the compatibility path ignores trailing SSE.
    let checked: LeadingError = serde_json::from_slice(&body[..values.byte_offset()]).ok()?;
    if [
        checked.error.code,
        checked.error.kind,
        checked.error.message,
    ]
    .iter()
    .all(|field| field.as_deref().is_none_or(|text| text.trim().is_empty()))
    {
        return None;
    }
    Some(first)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadingError {
    error: LeadingErrorFields,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadingErrorFields {
    #[serde(default)]
    code: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default, rename = "param")]
    _param: Option<String>,
}

use crate::runtime::CodexTurnStateScope;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response};
use reqwest::header::HeaderMap as UpstreamHeaderMap;

pub(super) const CODEX_TURN_STATE_HEADER: &str = "x-codex-turn-state";

const SESSION_HEADERS: &[&str] = &[
    "x-codex-session-id",
    "x-codex-parent-thread-id",
    "x-session-id",
    "session_id",
    "session-id",
    "thread-id",
];

fn client_session_id(headers: &HeaderMap) -> Option<&str> {
    SESSION_HEADERS.iter().find_map(|header_name| {
        let mut values = headers.get_all(*header_name).iter();
        let session_id = values.next()?.to_str().ok()?.trim();
        if values.next().is_some() {
            return None;
        }
        (!session_id.is_empty()
            && session_id.len() <= 256
            && !session_id.chars().any(char::is_control))
        .then_some(session_id)
    })
}

pub(super) fn request_scope<'a>(
    local_key_id: &'a str,
    headers: &'a HeaderMap,
    account_id: Option<&'a str>,
    model: &'a str,
    endpoint: &'a str,
) -> Option<CodexTurnStateScope<'a>> {
    Some(CodexTurnStateScope {
        local_key_id,
        session_id: client_session_id(headers)?,
        account_id: account_id?,
        model,
        endpoint,
    })
}

pub(super) fn relay_account_response_header(
    client_headers: &HeaderMap,
    upstream_headers: &UpstreamHeaderMap,
    response: &mut Response<Body>,
) {
    let Some(state) = upstream_headers.get(CODEX_TURN_STATE_HEADER) else {
        return;
    };
    let Some(_) = client_session_id(client_headers) else {
        return;
    };
    let Ok(state) = HeaderValue::from_bytes(state.as_bytes()) else {
        return;
    };
    if state.as_bytes().is_empty() || state.as_bytes().len() > 8192 {
        return;
    }
    response
        .headers_mut()
        .insert(HeaderName::from_static(CODEX_TURN_STATE_HEADER), state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_uses_native_codex_session_before_fallbacks() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-session-id",
            HeaderValue::from_static("codex-session-9"),
        );
        headers.insert("x-session-id", HeaderValue::from_static("session-42"));
        headers.insert(
            "x-codex-parent-thread-id",
            HeaderValue::from_static("thread-7"),
        );
        assert_eq!(client_session_id(&headers), Some("codex-session-9"));
    }

    #[test]
    fn invalid_session_id_is_not_used_for_provenance() {
        let mut headers = HeaderMap::new();
        let long_session = "x".repeat(257);
        headers.insert(
            "x-session-id",
            HeaderValue::from_bytes(long_session.as_bytes()).unwrap(),
        );
        assert!(client_session_id(&headers).is_none());
    }

    #[test]
    fn missing_session_id_is_not_eligible_for_turn_state() {
        let mut headers = HeaderMap::new();
        headers.insert(CODEX_TURN_STATE_HEADER, HeaderValue::from_static("state"));
        assert!(client_session_id(&headers).is_none());
    }

    #[test]
    fn soft_root_metadata_does_not_authorize_opaque_turn_state() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"session_id":"synthetic-root","thread_id":"child-a"}"#),
        );
        headers.insert(
            CODEX_TURN_STATE_HEADER,
            HeaderValue::from_static("unknown-state"),
        );
        assert!(request_scope("key", &headers, Some("account"), "model", "endpoint").is_none());
    }

    #[test]
    fn duplicate_session_headers_cannot_authorize_turn_state() {
        let mut headers = HeaderMap::new();
        headers.append("x-codex-session-id", HeaderValue::from_static("session-a"));
        headers.append("x-codex-session-id", HeaderValue::from_static("session-b"));
        assert!(request_scope("key", &headers, Some("account"), "model", "endpoint").is_none());
    }
}

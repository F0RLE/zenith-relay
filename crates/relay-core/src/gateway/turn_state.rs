use crate::GatewayRuntime;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response};
use reqwest::header::HeaderMap as UpstreamHeaderMap;

pub(super) const CODEX_TURN_STATE_HEADER: &str = "x-codex-turn-state";

const SESSION_HEADERS: &[&str] = &[
    "x-codex-parent-thread-id",
    "x-session-id",
    "session_id",
    "session-id",
    "thread-id",
];

fn client_session_id(headers: &HeaderMap) -> Option<String> {
    SESSION_HEADERS.iter().find_map(|name| {
        let value = headers.get(*name)?.to_str().ok()?.trim();
        (!value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control))
            .then(|| value.to_string())
    })
}

pub(super) fn guard_account_request(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    headers: &mut HeaderMap,
    account_id: &str,
    now_ms: u64,
) {
    if !headers.contains_key(CODEX_TURN_STATE_HEADER) {
        return;
    }
    let Some(session_id) = client_session_id(headers) else {
        headers.remove(CODEX_TURN_STATE_HEADER);
        return;
    };
    if !runtime.codex_turn_state_owned_by_account(local_key_id, &session_id, account_id, now_ms) {
        headers.remove(CODEX_TURN_STATE_HEADER);
    }
}

pub(super) fn relay_account_response_header(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    client_headers: &HeaderMap,
    account_id: &str,
    upstream_headers: &UpstreamHeaderMap,
    response: &mut Response<Body>,
    now_ms: u64,
) {
    let Some(state) = upstream_headers.get(CODEX_TURN_STATE_HEADER) else {
        return;
    };
    let Some(session_id) = client_session_id(client_headers) else {
        return;
    };
    let Ok(state) = HeaderValue::from_bytes(state.as_bytes()) else {
        return;
    };
    if state.as_bytes().is_empty() {
        return;
    }
    response
        .headers_mut()
        .insert(HeaderName::from_static(CODEX_TURN_STATE_HEADER), state);
    runtime.note_codex_turn_state(local_key_id, &session_id, account_id, now_ms);
}

pub(super) fn note_account_response_header(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    client_headers: &HeaderMap,
    account_id: &str,
    upstream_headers: &UpstreamHeaderMap,
    now_ms: u64,
) {
    let Some(state) = upstream_headers.get(CODEX_TURN_STATE_HEADER) else {
        return;
    };
    if state.as_bytes().is_empty() {
        return;
    }
    let Some(session_id) = client_session_id(client_headers) else {
        return;
    };
    runtime.note_codex_turn_state(local_key_id, &session_id, account_id, now_ms);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_uses_codex_thread_before_fallbacks() {
        let mut headers = HeaderMap::new();
        headers.insert("x-session-id", HeaderValue::from_static("session-42"));
        headers.insert(
            "x-codex-parent-thread-id",
            HeaderValue::from_static("thread-7"),
        );
        assert_eq!(client_session_id(&headers).as_deref(), Some("thread-7"));
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
}

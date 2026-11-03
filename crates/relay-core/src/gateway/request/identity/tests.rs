use super::*;
use axum::http::HeaderValue;
use serde_json::json;

#[test]
fn sibling_threads_share_root_affinity_but_not_context() {
    let headers = HeaderMap::new();
    let first = client_request_identity(
        &headers,
        &json!({
            "client_metadata": {"session_id": "synthetic-root", "thread_id": "child-a"}
        }),
    );
    let second = client_request_identity(
        &headers,
        &json!({
            "client_metadata": {"session_id": "synthetic-root", "thread_id": "child-b"}
        }),
    );
    let foreign = client_request_identity(
        &headers,
        &json!({
            "client_metadata": {"session_id": "foreign-root", "thread_id": "child-a"}
        }),
    );
    assert!(first.affinity_id.is_some());
    assert_eq!(first.affinity_id, second.affinity_id);
    assert_ne!(first.context_id, second.context_id);
    assert_ne!(first.affinity_id, foreign.affinity_id);
    assert_ne!(first.context_id, foreign.context_id);
    for id in [first.affinity_id, first.context_id].into_iter().flatten() {
        assert!(id.starts_with("client_"));
        assert_eq!(id.len(), 31);
        assert!(!id.contains("synthetic-root"));
        assert!(!id.contains("child-a"));
    }
}

#[test]
fn native_claude_session_stays_stable_across_turns() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-claude-code-session-id",
        HeaderValue::from_static("synthetic-claude-session"),
    );
    let first = client_request_identity(
        &headers,
        &json!({"messages": [{"role": "user", "content": "first"}]}),
    );
    let next = client_request_identity(
        &headers,
        &json!({"messages": [{"role": "user", "content": "next"}]}),
    );
    assert!(first.affinity_id.is_some());
    assert_eq!(first.affinity_id, next.affinity_id);
    assert_eq!(first.context_id, next.context_id);
}

#[test]
fn header_turn_metadata_precedes_legacy_and_body_hints() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(
            r#"{"session_id":"synthetic-root","thread_id":"synthetic-child"}"#,
        ),
    );
    headers.insert(
        "x-codex-session-id",
        HeaderValue::from_static("legacy-session"),
    );
    headers.insert("thread-id", HeaderValue::from_static("legacy-thread"));
    let identity = client_request_identity(
        &headers,
        &json!({
            "client_metadata": {"session_id": "body-root", "thread_id": "body-thread"}
        }),
    );
    let expected = client_request_identity(
        &HeaderMap::new(),
        &json!({
            "session_id": "synthetic-root", "thread_id": "synthetic-child"
        }),
    );
    assert_eq!(identity.affinity_id, expected.affinity_id);
    assert_eq!(identity.context_id, expected.context_id);
}

#[test]
fn per_request_metadata_overrides_connection_fallbacks() {
    let mut headers = HeaderMap::new();
    headers.insert("session-id", HeaderValue::from_static("connection-session"));
    headers.insert("thread-id", HeaderValue::from_static("connection-thread"));
    for field in TURN_METADATA_FIELDS {
        let mut body = json!({});
        body[*field] = json!(r#"{"session_id":"frame-session","thread_id":"frame-thread"}"#);
        let identity = client_request_identity(&headers, &body);
        let expected = client_request_identity(
            &HeaderMap::new(),
            &json!({
                "client_metadata": {"session_id": "frame-session", "thread_id": "frame-thread"}
            }),
        );
        assert_eq!(identity.affinity_id, expected.affinity_id, "{field}");
        assert_eq!(identity.context_id, expected.context_id, "{field}");
    }
}

#[test]
fn malformed_ambiguous_or_oversized_metadata_uses_bounded_fallbacks() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-session-id",
        HeaderValue::from_static("legacy-session"),
    );
    let expected = client_context_fingerprint(&headers);
    for metadata in [
        "not-json".to_string(),
        "[]".to_string(),
        r#"{"session_id":7}"#.to_string(),
        json!({"session_id": "x".repeat(257)}).to_string(),
        json!({"session_id": "invalid\nidentity"}).to_string(),
        json!({"session_id": "new", "padding": "x".repeat(8192)}).to_string(),
    ] {
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_str(&metadata).unwrap(),
        );
        assert_eq!(client_context_fingerprint(&headers), expected);
    }
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"session_id":"a"}"#),
    );
    headers.append(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"session_id":"b"}"#),
    );
    assert_eq!(client_context_fingerprint(&headers), expected);
}

#[test]
fn prompts_cache_keys_credentials_and_parent_hints_do_not_create_root_sessions() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer synthetic-local-key"),
    );
    headers.insert(
        "chatgpt-account-id",
        HeaderValue::from_static("untrusted-account"),
    );
    let identity = client_request_identity(
        &headers,
        &json!({
            "input": "synthetic text", "prompt_cache_key": "cache-not-a-session",
            "metadata": {"session_id": "unrelated-metadata"}
        }),
    );
    assert!(identity.context_id.is_none());
    assert!(identity.affinity_id.is_none());

    headers.insert("x-codex-session-id", HeaderValue::from_static("root"));
    let original = client_request_identity(&headers, &Value::Null);
    headers.insert(
        "x-codex-parent-thread-id",
        HeaderValue::from_static("other-parent"),
    );
    assert_eq!(
        original.affinity_id,
        client_request_identity(&headers, &Value::Null).affinity_id
    );
}

#[test]
fn claude_child_sessions_share_only_parent_affinity_and_keep_their_own_context() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-claude-code-session-id",
        HeaderValue::from_static("parent"),
    );
    let parent = client_request_identity(&headers, &Value::Null);
    headers.insert(
        "x-claude-code-session-id",
        HeaderValue::from_static("child"),
    );
    let request =
        json!({"metadata":{"user_id":r#"{"session_id":"child","parent_session_id":"parent"}"#}});
    let child = client_request_identity(&headers, &request);
    assert_eq!(child.affinity_id, parent.affinity_id);
    assert_ne!(child.context_id, parent.context_id);
    headers.insert(
        "x-codex-session-id",
        HeaderValue::from_static("explicit-root"),
    );
    assert_ne!(
        client_request_identity(&headers, &request).affinity_id,
        child.affinity_id
    );
}

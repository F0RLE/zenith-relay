use axum::http::{HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};

pub(in crate::gateway) const CLAUDE_CODE_SESSION_HEADER: &str = "x-claude-code-session-id";

/// Credentials supplied by a Relay client authenticate only the local
/// gateway. They must never be forwarded to a configured upstream source,
/// which authenticates with its own stored credential.
fn is_client_auth_header(name: &str) -> bool {
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

const FORWARDED_CODEX_HEADERS: &[&str] = &[
    "openai-beta",
    "originator",
    "session-id",
    "session_id",
    "thread-id",
    "traceparent",
    "tracestate",
    "user-agent",
    "version",
    "x-claude-code-session-id",
    "x-client-request-id",
    "x-codex-beta-features",
    "x-codex-installation-id",
    "x-codex-parent-thread-id",
    "x-codex-turn-metadata",
    "x-codex-turn-state",
    "x-codex-window-id",
    "x-oai-attestation",
    "x-openai-memgen-request",
    "x-openai-subagent",
    "x-responsesapi-include-timing-metrics",
    "x-session-id",
];

const CLIENT_CONTEXT_HEADERS: &[&str] = &[
    "x-session-id",
    "session_id",
    "session-id",
    "thread-id",
    "x-codex-parent-thread-id",
    "x-codex-window-id",
    "x-codex-installation-id",
];

/// Returns a stable, privacy-safe identifier for the client stream that sent
/// a request. Raw session, thread, installation, and window values never
/// leave this function.
pub(in crate::gateway) fn client_context_fingerprint(client_headers: &HeaderMap) -> Option<String> {
    let mut digest = Sha256::new();
    let mut found = false;
    for &name in CLIENT_CONTEXT_HEADERS {
        let Some(value) = client_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        found = true;
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    found.then(|| format!("client_{}", hex::encode(&digest.finalize()[..12])))
}

pub(in crate::gateway) fn forwarded_codex_headers(
    client_headers: &HeaderMap,
    fallback_session_id: &str,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for &name in FORWARDED_CODEX_HEADERS {
        if let Some(value) = client_headers.get(name) {
            headers.insert(HeaderName::from_static(name), value.clone());
        }
    }
    if !headers.contains_key(CLAUDE_CODE_SESSION_HEADER) {
        let session_id = ["session_id", "x-session-id", "session-id", "thread-id"]
            .iter()
            .find_map(|name| client_headers.get(*name))
            .cloned()
            .or_else(|| HeaderValue::from_str(fallback_session_id).ok());
        if let Some(session_id) = session_id {
            headers.insert(
                HeaderName::from_static(CLAUDE_CODE_SESSION_HEADER),
                session_id,
            );
        }
    }
    headers
}

/// A Responses-to-Messages bridge receives a Codex/Responses client request,
/// not a native Anthropic client request. Carry only the metadata that has a
/// defined Messages-side meaning; forwarding OpenAI/Codex headers would leak
/// private client state into an unrelated upstream contract.
pub(in crate::gateway) fn forwarded_bridge_messages_headers(
    client_headers: &HeaderMap,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in ["user-agent", CLAUDE_CODE_SESSION_HEADER] {
        if let Some(value) = client_headers.get(name) {
            headers.insert(HeaderName::from_static(name), value.clone());
        }
    }
    headers
}

/// A Responses-to-Gemini bridge has no Messages session contract. Keep only a
/// harmless client identity header and never forward Claude/OpenAI metadata.
pub(in crate::gateway) fn forwarded_bridge_gemini_headers(client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = client_headers.get("user-agent") {
        headers.insert(HeaderName::from_static("user-agent"), value.clone());
    }
    headers
}

/// For native Messages routes, forward only headers that belong to the
/// Anthropic contract. This avoids leaking Codex/OpenAI request metadata into
/// a different upstream protocol while retaining the version and session
/// details needed by Claude Code.
pub(in crate::gateway) fn forwarded_messages_headers(client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in client_headers {
        let name = name.as_str();
        let is_messages_metadata = name == "user-agent"
            || name.starts_with("anthropic-")
            || name.starts_with("x-claude-")
            || name.starts_with("x-stainless-");
        if is_messages_metadata && !is_client_auth_header(name) {
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).expect("request header name is valid"),
                value.clone(),
            );
        }
    }
    headers
        .entry(HeaderName::from_static("anthropic-version"))
        .or_insert_with(|| HeaderValue::from_static("2023-06-01"));
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::AUTHORIZATION;

    #[test]
    fn forwarded_codex_headers_keep_session_identity_and_drop_secrets() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("x-session-id", HeaderValue::from_static("session-42"));
        client_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer local-secret"),
        );
        client_headers.insert("cookie", HeaderValue::from_static("session=secret"));
        client_headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("private-account"),
        );

        let forwarded = forwarded_codex_headers(&client_headers, "relay-request");
        assert_eq!(forwarded["x-session-id"], "session-42");
        assert_eq!(forwarded[CLAUDE_CODE_SESSION_HEADER], "session-42");
        assert!(!forwarded.contains_key(AUTHORIZATION));
        assert!(!forwarded.contains_key("cookie"));
        assert!(!forwarded.contains_key("chatgpt-account-id"));

        let synthesized = forwarded_codex_headers(&HeaderMap::new(), "relay-request");
        assert_eq!(synthesized[CLAUDE_CODE_SESSION_HEADER], "relay-request");
    }

    #[test]
    fn client_context_fingerprint_is_stable_and_ignores_untrusted_headers() {
        let mut first = HeaderMap::new();
        first.insert("thread-id", HeaderValue::from_static("thread-42"));
        first.insert("x-session-id", HeaderValue::from_static("session-42"));
        first.insert("authorization", HeaderValue::from_static("secret-a"));
        first.insert("cookie", HeaderValue::from_static("session=secret-a"));
        first.insert("user-agent", HeaderValue::from_static("codex-a"));

        let mut second = first.clone();
        second.insert("authorization", HeaderValue::from_static("secret-b"));
        second.insert("cookie", HeaderValue::from_static("session=secret-b"));
        second.insert("user-agent", HeaderValue::from_static("codex-b"));

        let fingerprint = client_context_fingerprint(&first).unwrap();
        assert_eq!(
            Some(fingerprint.clone()),
            client_context_fingerprint(&second)
        );
        assert!(fingerprint.starts_with("client_"));
        assert_eq!(fingerprint.len(), "client_".len() + 24);

        let mut different_thread = first.clone();
        different_thread.insert("thread-id", HeaderValue::from_static("thread-43"));
        assert_ne!(
            Some(fingerprint),
            client_context_fingerprint(&different_thread)
        );
        assert!(serde_json::to_string(&client_context_fingerprint(&first))
            .unwrap()
            .contains("client_"));
        assert!(!serde_json::to_string(&client_context_fingerprint(&first))
            .unwrap()
            .contains("thread-42"));
    }

    #[test]
    fn forwarded_messages_headers_keep_protocol_metadata_and_drop_client_credentials() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        client_headers.insert(
            "anthropic-beta",
            HeaderValue::from_static("fine-grained-tool"),
        );
        client_headers.insert(
            CLAUDE_CODE_SESSION_HEADER,
            HeaderValue::from_static("session-42"),
        );
        client_headers.insert("x-stainless-lang", HeaderValue::from_static("rust"));
        client_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer relay-local-secret"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("relay-local-secret"));
        client_headers.insert(
            "anthropic-api-key",
            HeaderValue::from_static("client-anthropic-secret"),
        );
        client_headers.insert(
            "openai-api-key",
            HeaderValue::from_static("client-openai-secret"),
        );
        client_headers.insert(
            "x-goog-api-key",
            HeaderValue::from_static("client-google-secret"),
        );
        client_headers.insert("cookie", HeaderValue::from_static("session=secret"));

        let forwarded = forwarded_messages_headers(&client_headers);

        assert_eq!(forwarded["anthropic-version"], "2023-06-01");
        assert_eq!(forwarded["anthropic-beta"], "fine-grained-tool");
        assert_eq!(forwarded[CLAUDE_CODE_SESSION_HEADER], "session-42");
        assert_eq!(forwarded["x-stainless-lang"], "rust");
        for name in [
            "authorization",
            "x-api-key",
            "anthropic-api-key",
            "openai-api-key",
            "x-goog-api-key",
            "cookie",
        ] {
            assert!(
                !forwarded.contains_key(name),
                "{name} must not be forwarded"
            );
        }
    }

    #[test]
    fn bridged_messages_headers_do_not_forward_codex_metadata() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("user-agent", HeaderValue::from_static("codex-test"));
        client_headers.insert(
            CLAUDE_CODE_SESSION_HEADER,
            HeaderValue::from_static("session-42"),
        );
        client_headers.insert(
            "x-oai-attestation",
            HeaderValue::from_static("private-attestation"),
        );
        client_headers.insert(
            "x-openai-memgen-request",
            HeaderValue::from_static("private-memgen"),
        );
        client_headers.insert("openai-beta", HeaderValue::from_static("responses=v1"));
        client_headers.insert("anthropic-beta", HeaderValue::from_static("tools"));

        let forwarded = forwarded_bridge_messages_headers(&client_headers);

        assert_eq!(forwarded["user-agent"], "codex-test");
        assert_eq!(forwarded[CLAUDE_CODE_SESSION_HEADER], "session-42");
        for name in [
            "x-oai-attestation",
            "x-openai-memgen-request",
            "openai-beta",
            "anthropic-beta",
        ] {
            assert!(
                !forwarded.contains_key(name),
                "{name} must not cross the Responses-to-Messages boundary"
            );
        }
    }
}

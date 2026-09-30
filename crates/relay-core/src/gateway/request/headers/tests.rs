use super::*;
use crate::providers::chatgpt::CODEX_CLIENT_VERSION;
use axum::http::header::AUTHORIZATION;

#[test]
fn forwarded_codex_headers_keep_session_identity_and_drop_secrets() {
    let mut client_headers = HeaderMap::new();
    client_headers.insert(
        "x-codex-session-id",
        HeaderValue::from_static("codex-session-42"),
    );
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
    assert_eq!(forwarded["x-codex-session-id"], "codex-session-42");
    assert_eq!(forwarded[CLAUDE_CODE_SESSION_HEADER], "codex-session-42");
    assert!(!forwarded.contains_key(AUTHORIZATION));
    assert!(!forwarded.contains_key("cookie"));
    assert!(!forwarded.contains_key("chatgpt-account-id"));

    let synthesized = forwarded_codex_headers(&HeaderMap::new(), "relay-request");
    assert_eq!(synthesized[CLAUDE_CODE_SESSION_HEADER], "relay-request");
}

#[test]
fn managed_codex_client_detection_does_not_claim_generic_api_requests() {
    let generic = HeaderMap::new();
    assert!(!is_managed_codex_client(&generic));

    for (name, value) in [
        ("originator", "my-codex-integration"),
        ("originator", "chatgpt-client"),
        ("user-agent", "my-chatgpt-wrapper/1.0"),
        ("user-agent", "my Codex Desktop/1.0 wrapper"),
    ] {
        let mut generic_named_client = HeaderMap::new();
        generic_named_client.insert(name, HeaderValue::from_static(value));
        assert!(
            !is_managed_codex_client(&generic_named_client),
            "{name}: {value} must remain client-owned"
        );
    }

    let mut codex = HeaderMap::new();
    codex.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    assert!(is_managed_codex_client(&codex));

    let mut desktop_codex = HeaderMap::new();
    desktop_codex.insert(
        "user-agent",
        HeaderValue::from_str(&format!("Codex Desktop/{CODEX_CLIENT_VERSION} (Windows)")).unwrap(),
    );
    assert!(is_managed_codex_client(&desktop_codex));

    let mut metadata_only = HeaderMap::new();
    metadata_only.insert("x-codex-session-id", HeaderValue::from_static("session-1"));
    assert!(is_managed_codex_client(&metadata_only));

    let mut chatgpt = HeaderMap::new();
    chatgpt.insert("user-agent", HeaderValue::from_static("ChatGPTDesktop/1.0"));
    assert!(is_managed_codex_client(&chatgpt));
}

#[test]
fn codex_client_version_accepts_only_valid_announced_versions() {
    let mut headers = HeaderMap::new();
    headers.insert("version", HeaderValue::from_static(CODEX_CLIENT_VERSION));
    assert_eq!(codex_client_version(&headers), Some(CODEX_CLIENT_VERSION));
    headers.insert("version", HeaderValue::from_static("not a version"));
    assert_eq!(codex_client_version(&headers), None);
    headers.remove("version");
    headers.insert(
        "user-agent",
        HeaderValue::from_str(&format!(
            "codex-tui/{CODEX_CLIENT_VERSION} (Windows NT 10.0; x64)"
        ))
        .unwrap(),
    );
    assert_eq!(codex_client_version(&headers), Some(CODEX_CLIENT_VERSION));

    headers.insert(
        "user-agent",
        HeaderValue::from_static("Codex Desktop/0.166.0 (Windows; x86_64)"),
    );
    assert_eq!(codex_client_version(&headers), Some("0.166.0"));
    headers.insert(
        "user-agent",
        HeaderValue::from_static("codex_cli_rs/0.167.0"),
    );
    assert_eq!(codex_client_version(&headers), Some("0.167.0"));
    headers.insert(
        "user-agent",
        HeaderValue::from_static("wrapper Codex Desktop/0.168.0"),
    );
    assert_eq!(codex_client_version(&headers), None);
}

#[test]
fn codex_routing_hint_is_rebuilt_for_each_explicit_speed_tier() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-routing-hint",
        HeaderValue::from_static("model=spoofed;tier=priority"),
    );
    apply_codex_routing_hint(&mut headers, "gpt-5.4", DefaultServiceTier::Fast);
    assert_eq!(
        headers["x-codex-routing-hint"],
        "model=gpt-5.4;tier=priority"
    );

    apply_codex_routing_hint(&mut headers, "gpt-5.6-sol", DefaultServiceTier::Ultrafast);
    assert_eq!(
        headers["x-codex-routing-hint"],
        "model=gpt-5.6-sol;tier=ultrafast"
    );

    apply_codex_routing_hint(&mut headers, "gpt-5.4", DefaultServiceTier::Standard);
    assert!(!headers.contains_key("x-codex-routing-hint"));
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
    different_thread.insert("x-session-id", HeaderValue::from_static("session-43"));
    assert_ne!(
        Some(fingerprint),
        client_context_fingerprint(&different_thread)
    );

    let mut different_window = first.clone();
    different_window.insert("x-codex-window-id", HeaderValue::from_static("window-2"));
    assert_eq!(
        client_context_fingerprint(&first),
        client_context_fingerprint(&different_window)
    );
    assert!(serde_json::to_string(&client_context_fingerprint(&first))
        .unwrap()
        .contains("client_"));
    assert!(!serde_json::to_string(&client_context_fingerprint(&first))
        .unwrap()
        .contains("thread-42"));
}

#[test]
fn native_codex_session_takes_precedence_for_client_affinity() {
    let mut first = HeaderMap::new();
    first.insert(
        "x-codex-session-id",
        HeaderValue::from_static("codex-session-42"),
    );
    first.insert("x-session-id", HeaderValue::from_static("legacy-session-a"));

    let mut changed_legacy = first.clone();
    changed_legacy.insert("x-session-id", HeaderValue::from_static("legacy-session-b"));

    let mut changed_codex = first.clone();
    changed_codex.insert(
        "x-codex-session-id",
        HeaderValue::from_static("codex-session-43"),
    );

    assert_eq!(
        client_context_fingerprint(&first),
        client_context_fingerprint(&changed_legacy)
    );
    assert_ne!(
        client_context_fingerprint(&first),
        client_context_fingerprint(&changed_codex)
    );
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

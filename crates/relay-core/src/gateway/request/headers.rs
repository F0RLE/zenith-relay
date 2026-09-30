use crate::providers::chatgpt::valid_codex_client_version;
use crate::runtime::DefaultServiceTier;
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
    "x-codex-session-id",
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
    "x-codex-session-id",
    "x-session-id",
    "session_id",
    "session-id",
    "thread-id",
    "x-codex-parent-thread-id",
    "x-codex-window-id",
    "x-codex-installation-id",
];

const MANAGED_CODEX_USER_AGENT_PREFIXES: &[&str] = &[
    "codex desktop/",
    "codex-tui/",
    "codex_cli_rs/",
    "chatgptdesktop/",
];

const MANAGED_CODEX_ORIGINATORS: &[&str] = &[
    "codex_cli_rs",
    "codex-tui",
    "codex desktop",
    "chatgpt desktop",
    "chatgptdesktop",
];

const CODEX_VERSION_USER_AGENT_PREFIXES: &[&str] =
    &["codex desktop/", "codex-tui/", "codex_cli_rs/"];

/// Returns a stable, privacy-safe identifier for the client stream that sent
/// a request. Raw session, thread, installation, and window values never
/// leave this function.
pub(in crate::gateway) fn client_context_fingerprint(client_headers: &HeaderMap) -> Option<String> {
    let mut digest = Sha256::new();
    let (name, value) = CLIENT_CONTEXT_HEADERS.iter().find_map(|&name| {
        client_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| (name, value))
    })?;
    digest.update(name.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    Some(format!("client_{}", hex::encode(&digest.finalize()[..12])))
}

/// Identifies Relay-managed Codex or ChatGPT traffic without treating an
/// ordinary OpenAI-compatible API call as a managed client request. The result
/// controls only the local pool's speed policy; it is never forwarded
/// upstream.
pub(in crate::gateway) fn is_managed_codex_client(headers: &HeaderMap) -> bool {
    if headers
        .keys()
        .any(|name| name.as_str().starts_with("x-codex-"))
        || headers.contains_key("x-openai-internal-codex-responses-lite")
        || headers.contains_key("x-openai-subagent")
    {
        return true;
    }
    let originator_is_managed = headers
        .get("originator")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .is_some_and(|value| {
            MANAGED_CODEX_ORIGINATORS
                .iter()
                .any(|identity| value.eq_ignore_ascii_case(identity))
        });
    if originator_is_managed {
        return true;
    }

    headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| {
            value == "codex_cli_rs"
                || MANAGED_CODEX_USER_AGENT_PREFIXES
                    .iter()
                    .any(|prefix| value.starts_with(prefix))
        })
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
        let session_id = [
            "x-codex-session-id",
            "session_id",
            "x-session-id",
            "session-id",
            "thread-id",
        ]
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

pub(in crate::gateway) fn codex_client_version(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("version")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| valid_codex_client_version(value))
        .or_else(|| {
            headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok())
                .and_then(codex_version_from_user_agent)
        })
}

fn codex_version_from_user_agent(value: &str) -> Option<&str> {
    let value = value.trim();
    let lowercase = value.to_ascii_lowercase();
    CODEX_VERSION_USER_AGENT_PREFIXES.iter().find_map(|prefix| {
        lowercase
            .strip_prefix(prefix)
            .and_then(|_| value.get(prefix.len()..))
            .and_then(|value| value.split_whitespace().next())
            .map(str::trim)
            .filter(|value| valid_codex_client_version(value))
    })
}

/// Rebuilds the Codex routing hint for the concrete OAuth route selected by
/// the scheduler. The hint is deliberately not accepted from the client: a
/// retry may select another model/route, so it must be regenerated per
/// attempt alongside the effective service tier.
pub(in crate::gateway) fn apply_codex_routing_hint(
    headers: &mut HeaderMap,
    model: &str,
    service_tier: DefaultServiceTier,
) {
    let name = HeaderName::from_static("x-codex-routing-hint");
    headers.remove(&name);
    let tier = match service_tier {
        DefaultServiceTier::Standard => return,
        DefaultServiceTier::Fast => "priority",
        DefaultServiceTier::Ultrafast => "ultrafast",
    };
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    let Ok(value) = HeaderValue::from_str(&format!("model={model};tier={tier}")) else {
        return;
    };
    headers.insert(name, value);
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
mod tests;

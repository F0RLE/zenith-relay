use crate::providers::chatgpt::valid_codex_client_version;
use crate::runtime::DefaultServiceTier;
use axum::http::{HeaderMap, HeaderName, HeaderValue};

pub(in crate::gateway) const CLAUDE_CODE_SESSION_HEADER: &str = "x-claude-code-session-id";

/// Credentials supplied by a Relay client authenticate only the local
/// gateway. They must never be forwarded to a configured upstream source,
/// which authenticates with its own stored credential.
fn is_client_auth_header(header_name: &str) -> bool {
    matches!(
        header_name,
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-auth-token"
            | "x-api-token"
    ) || header_name.ends_with("-api-key")
}

// Connection options are hop-by-hop even when they name an otherwise allowed
// protocol extension. Never restore them on the next upstream connection.
fn is_connection_header(client_headers: &HeaderMap, header_name: &str) -> bool {
    client_headers.get_all("connection").iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|option| option.trim().eq_ignore_ascii_case(header_name))
        })
    })
}

const FORWARDED_RESPONSES_HEADERS: &[&str] = &[
    "openai-beta",
    "originator",
    "session-id",
    "session_id",
    "thread-id",
    "traceparent",
    "tracestate",
    "user-agent",
    "version",
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

/// Identifies Relay-managed Codex or ChatGPT traffic without treating an
/// ordinary OpenAI-compatible API call as a managed client request. The result
/// controls only the local pool's speed policy; it is never forwarded
/// upstream.
pub(in crate::gateway) fn is_managed_codex_client(headers: &HeaderMap) -> bool {
    if headers
        .keys()
        .any(|header_name| header_name.as_str().starts_with("x-codex-"))
        || headers.contains_key("x-openai-internal-codex-responses-lite")
        || headers.contains_key("x-openai-subagent")
    {
        return true;
    }
    let originator_is_managed = headers
        .get("originator")
        .and_then(|header_value| header_value.to_str().ok())
        .map(str::trim)
        .is_some_and(|originator| {
            MANAGED_CODEX_ORIGINATORS
                .iter()
                .any(|identity| originator.eq_ignore_ascii_case(identity))
        });
    if originator_is_managed {
        return true;
    }

    headers
        .get("user-agent")
        .and_then(|header_value| header_value.to_str().ok())
        .map(|user_agent| user_agent.trim().to_ascii_lowercase())
        .is_some_and(|user_agent| {
            user_agent == "codex_cli_rs"
                || MANAGED_CODEX_USER_AGENT_PREFIXES
                    .iter()
                    .any(|prefix| user_agent.starts_with(prefix))
        })
}

pub(in crate::gateway) fn forwarded_responses_headers(client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for &header_name in FORWARDED_RESPONSES_HEADERS {
        if is_connection_header(client_headers, header_name) {
            continue;
        }
        // Session/state identity is a singleton contract, unlike beta feature
        // lists. Do not let upstream choose a different duplicate from Relay.
        if matches!(
            header_name,
            "session-id"
                | "session_id"
                | "thread-id"
                | "x-codex-session-id"
                | "x-codex-parent-thread-id"
                | "x-codex-turn-metadata"
                | "x-codex-turn-state"
                | "x-session-id"
        ) && client_headers.get_all(header_name).iter().count() != 1
        {
            continue;
        }
        for header_value in client_headers.get_all(header_name) {
            headers.append(HeaderName::from_static(header_name), header_value.clone());
        }
    }
    headers
}

pub(in crate::gateway) fn forwarded_codex_headers(
    client_headers: &HeaderMap,
    fallback_session_id: &str,
) -> HeaderMap {
    let mut headers = forwarded_responses_headers(client_headers);
    let session_id = client_headers
        .get(CLAUDE_CODE_SESSION_HEADER)
        .filter(|_| !is_connection_header(client_headers, CLAUDE_CODE_SESSION_HEADER))
        .filter(|_| {
            client_headers
                .get_all(CLAUDE_CODE_SESSION_HEADER)
                .iter()
                .count()
                == 1
        })
        .or_else(|| {
            [
                "x-codex-session-id",
                "session_id",
                "x-session-id",
                "session-id",
                "thread-id",
            ]
            .iter()
            .filter(|header_name| !is_connection_header(client_headers, header_name))
            .filter(|header_name| client_headers.get_all(**header_name).iter().count() == 1)
            .find_map(|header_name| client_headers.get(*header_name))
        })
        .cloned()
        .or_else(|| HeaderValue::from_str(fallback_session_id).ok());
    if let Some(session_id) = session_id {
        headers.insert(
            HeaderName::from_static(CLAUDE_CODE_SESSION_HEADER),
            session_id,
        );
    }
    headers
}

pub(in crate::gateway) fn codex_client_version(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("version")
        .and_then(|header_value| header_value.to_str().ok())
        .map(str::trim)
        .filter(|version| valid_codex_client_version(version))
        .or_else(|| {
            headers
                .get("user-agent")
                .and_then(|header_value| header_value.to_str().ok())
                .and_then(codex_version_from_user_agent)
        })
}

fn codex_version_from_user_agent(user_agent_text: &str) -> Option<&str> {
    let user_agent_text = user_agent_text.trim();
    let lowercase = user_agent_text.to_ascii_lowercase();
    CODEX_VERSION_USER_AGENT_PREFIXES.iter().find_map(|prefix| {
        lowercase
            .strip_prefix(prefix)
            .and_then(|_| user_agent_text.get(prefix.len()..))
            .and_then(|version_tail| version_tail.split_whitespace().next())
            .map(str::trim)
            .filter(|version| valid_codex_client_version(version))
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
    let header_name = HeaderName::from_static("x-codex-routing-hint");
    headers.remove(&header_name);
    let tier = match service_tier {
        DefaultServiceTier::Standard => return,
        DefaultServiceTier::Fast => "priority",
        DefaultServiceTier::Ultrafast => "ultrafast",
    };
    let model_id = model.trim();
    if model_id.is_empty() {
        return;
    }
    let Ok(routing_hint) = HeaderValue::from_str(&format!("model={model_id};tier={tier}")) else {
        return;
    };
    headers.insert(header_name, routing_hint);
}

/// A Responses-to-Messages bridge receives a Codex/Responses client request,
/// not a native Anthropic client request. Carry only the metadata that has a
/// defined Messages-side meaning; forwarding OpenAI/Codex headers would leak
/// private client state into an unrelated upstream contract.
pub(in crate::gateway) fn forwarded_bridge_messages_headers(
    client_headers: &HeaderMap,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for header_name in ["user-agent", CLAUDE_CODE_SESSION_HEADER] {
        if is_connection_header(client_headers, header_name) {
            continue;
        }
        if header_name == CLAUDE_CODE_SESSION_HEADER
            && client_headers.get_all(header_name).iter().count() != 1
        {
            continue;
        }
        if let Some(header_value) = client_headers.get(header_name) {
            headers.insert(HeaderName::from_static(header_name), header_value.clone());
        }
    }
    headers
}

/// A Responses-to-Gemini bridge has no Messages session contract. Keep only a
/// harmless client identity header and never forward Claude/OpenAI metadata.
pub(in crate::gateway) fn forwarded_bridge_gemini_headers(client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(user_agent) = client_headers
        .get("user-agent")
        .filter(|_| !is_connection_header(client_headers, "user-agent"))
    {
        headers.insert(HeaderName::from_static("user-agent"), user_agent.clone());
    }
    headers
}

/// For native Messages routes, forward only headers that belong to the
/// Anthropic contract. This avoids leaking Codex/OpenAI request metadata into
/// a different upstream protocol while retaining the version and session
/// details needed by Claude Code.
pub(in crate::gateway) fn forwarded_messages_headers(client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (header_name, header_value) in client_headers {
        let header_name = header_name.as_str();
        let is_messages_metadata = header_name == "user-agent"
            || header_name.starts_with("anthropic-")
            || header_name.starts_with("x-claude-")
            || header_name.starts_with("x-stainless-");
        if is_messages_metadata
            && !is_client_auth_header(header_name)
            && !is_connection_header(client_headers, header_name)
            && (header_name != CLAUDE_CODE_SESSION_HEADER
                || client_headers.get_all(header_name).iter().count() == 1)
        {
            headers.append(
                HeaderName::from_bytes(header_name.as_bytes())
                    .expect("request header name is valid"),
                header_value.clone(),
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

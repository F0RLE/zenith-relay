//! Client hints are soft routing/observation identities, never continuation
//! authorization. Root affinity and a child thread's context stay separate.

use axum::http::HeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};
mod history;
pub(in crate::gateway) use history::history_affinity_fingerprint;

const SESSION_HEADERS: &[&str] = &[
    "x-codex-session-id",
    "x-session-id",
    "session_id",
    "session-id",
    "x-claude-code-session-id",
];
const FALLBACK_HEADERS: &[&str] = &[
    "thread-id",
    "x-codex-parent-thread-id",
    "x-codex-window-id",
    "x-codex-installation-id",
];
const TURN_METADATA_FIELDS: &[&str] = &["turnMetadata", "turn_metadata", "x-codex-turn-metadata"];

pub(in crate::gateway) struct ClientRequestIdentity {
    pub(in crate::gateway) context_id: Option<String>,
    pub(in crate::gateway) affinity_id: Option<String>,
}

pub(in crate::gateway) fn client_context_fingerprint(headers: &HeaderMap) -> Option<String> {
    client_request_identity(headers, &Value::Null).context_id
}

pub(in crate::gateway) fn client_request_identity(
    headers: &HeaderMap,
    request: &Value,
) -> ClientRequestIdentity {
    let header_metadata = single_header(headers, "x-codex-turn-metadata").and_then(turn_metadata);
    let client_metadata = request.get("client_metadata");
    let body_metadata = [Some(request), client_metadata]
        .into_iter()
        .flatten()
        .flat_map(|object| {
            TURN_METADATA_FIELDS
                .iter()
                .filter_map(move |field| object.get(*field))
        })
        .filter_map(Value::as_str)
        .find_map(turn_metadata);
    let logical_field = |field| {
        header_metadata
            .as_ref()
            .and_then(|metadata| identity_field(metadata, field))
            .or_else(|| client_metadata.and_then(|metadata| identity_field(metadata, field)))
            .or_else(|| {
                body_metadata
                    .as_ref()
                    .and_then(|metadata| identity_field(metadata, field))
            })
    };
    let root = logical_field("session_id")
        .map(|value| ("x-codex-session-id", value))
        .or_else(|| header_identity(headers, SESSION_HEADERS))
        .or_else(|| {
            identity_field(request, "session_id").map(|value| ("x-codex-session-id", value))
        });
    let thread = logical_field("thread_id")
        .or_else(|| single_header(headers, "thread-id").and_then(valid_identity))
        .or_else(|| identity_field(request, "thread_id"));
    let fallback = root.or_else(|| header_identity(headers, FALLBACK_HEADERS));
    let mut affinity_id = fallback
        .map(|(kind, value)| fingerprint(kind, value))
        .or_else(|| thread.map(|value| fingerprint("thread-id", value)));
    let mut context_id = match (root, thread) {
        // Preserve the old session fingerprint for single-thread clients.
        (Some((_, session)), Some(thread)) if session != thread => Some(fingerprint(
            "thread",
            &format!("{}:{session}{thread}", session.len()),
        )),
        (None, Some(thread)) => Some(fingerprint("thread-id", thread)),
        _ => affinity_id.clone(),
    };
    // Observed Claude Code metadata, not an Anthropic account identity. A
    // child/parent pair is only a soft hint; never replace an explicit Codex root.
    if logical_field("session_id").is_none()
        && header_identity(
            headers,
            &[
                "x-codex-session-id",
                "x-session-id",
                "session_id",
                "session-id",
            ],
        )
        .is_none()
    {
        if let Some(metadata) = request
            .pointer("/metadata/user_id")
            .and_then(Value::as_str)
            .and_then(turn_metadata)
        {
            if let (Some(child), Some(parent)) = (
                identity_field(&metadata, "session_id"),
                identity_field(&metadata, "parent_session_id"),
            ) {
                let header_child =
                    single_header(headers, "x-claude-code-session-id").and_then(valid_identity);
                if child != parent && header_child.is_none_or(|header| header == child) {
                    context_id = Some(fingerprint("x-claude-code-session-id", child));
                    affinity_id = Some(fingerprint("x-claude-code-session-id", parent));
                }
            }
        }
    }
    ClientRequestIdentity {
        context_id,
        affinity_id,
    }
}

fn valid_identity(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control))
        .then_some(value)
}

fn identity_field<'a>(object: &'a Value, field: &str) -> Option<&'a str> {
    valid_identity(object.get(field)?.as_str()?)
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
}

fn header_identity<'a>(
    headers: &'a HeaderMap,
    names: &[&'static str],
) -> Option<(&'static str, &'a str)> {
    names.iter().find_map(|&name| {
        single_header(headers, name)
            .and_then(valid_identity)
            .map(|value| (name, value))
    })
}

fn turn_metadata(value: &str) -> Option<Value> {
    if value.len() > 8192 {
        return None;
    }
    serde_json::from_str(value).ok().filter(Value::is_object)
}

fn fingerprint(kind: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    format!("client_{}", hex::encode(&digest.finalize()[..12]))
}

#[cfg(test)]
mod tests;

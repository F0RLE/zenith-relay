use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Map, Value};

const MAX_METADATA_BYTES: usize = 8192;
const TURN_STATE_FIELDS: &[&str] = &["turnState", "turn_state", "x-codex-turn-state"];
const TURN_METADATA_FIELDS: &[&str] = &["turnMetadata", "turn_metadata", "x-codex-turn-metadata"];

// These fields cannot bind a request to an account chosen by the client.
// Authorization and account identity belong to the selected executor.
const CLIENT_IDENTITY_FIELDS: &[&str] = &[
    "account_id",
    "accountId",
    "chatgpt_account_id",
    "chatgptAccountId",
    "chatgpt-account-id",
    "project_id",
    "projectId",
    "openai-project",
    "openai-organization",
    "authorization",
    "proxy-authorization",
    "api_key",
    "apiKey",
    "access_token",
    "accessToken",
    "session_token",
    "sessionToken",
    "refresh_token",
    "refreshToken",
    "id_token",
    "idToken",
    "token",
    "cookie",
    "cookies",
    "cookie_header",
    "cookieHeader",
    "cf_clearance",
    "x-oai-attestation",
];

// Only the canonical top-level reference goes through continuation admission.
// Duplicates in client/turn metadata must not bypass that ownership check.
const METADATA_RESPONSE_FIELDS: &[&str] = &[
    "previous_response_id",
    "previousResponseId",
    "response_id",
    "responseId",
    "conversation",
];

pub(in crate::runtime) fn guard_headers(
    headers: &mut HeaderMap,
    matches_state: &impl Fn(&[u8]) -> bool,
) {
    let mut states = headers.get_all("x-codex-turn-state").iter();
    let owned_state = states
        .next()
        .is_some_and(|state| matches_state(state.as_bytes()))
        && states.next().is_none();
    if !owned_state {
        headers.remove("x-codex-turn-state");
    }
    if let Some(metadata) = headers.get("x-codex-turn-metadata").cloned() {
        let scoped = metadata
            .to_str()
            .ok()
            .and_then(|raw| scoped_turn_metadata(raw, matches_state))
            .and_then(|raw| HeaderValue::from_str(&raw).ok());
        headers.remove("x-codex-turn-metadata");
        if let Some(scoped) = scoped {
            headers.insert("x-codex-turn-metadata", scoped);
        }
    }
}

pub(in crate::runtime) fn guard_payload(
    payload: &mut Value,
    matches_state: &impl Fn(&[u8]) -> bool,
) -> bool {
    let Some(object) = payload.as_object_mut() else {
        return false;
    };
    let mut changed = guard_fields(object, matches_state);
    if let Some(metadata) = object.get_mut("client_metadata") {
        match metadata.as_object_mut() {
            Some(metadata) => {
                changed |= guard_fields(metadata, matches_state);
                changed |= remove_fields(metadata, METADATA_RESPONSE_FIELDS);
                changed |= metadata.remove("parent_response_id").is_some();
                if metadata.is_empty() {
                    object.remove("client_metadata");
                    changed = true;
                }
            }
            None => {
                // Opaque client metadata cannot be checked for account state.
                object.remove("client_metadata");
                changed = true;
            }
        }
    }
    changed
}

fn guard_fields(object: &mut Map<String, Value>, matches_state: &impl Fn(&[u8]) -> bool) -> bool {
    let mut changed = remove_fields(object, CLIENT_IDENTITY_FIELDS);
    changed |= guard_state_fields(object, matches_state);
    for field in TURN_METADATA_FIELDS {
        let Some(value) = object.get_mut(*field) else {
            continue;
        };
        let scoped = value
            .as_str()
            .and_then(|raw| scoped_turn_metadata(raw, matches_state));
        match scoped {
            Some(scoped) if value.as_str() != Some(scoped.as_str()) => {
                *value = Value::String(scoped);
                changed = true;
            }
            Some(_) => {}
            None => {
                object.remove(*field);
                changed = true;
            }
        }
    }
    changed
}

fn guard_state_fields(
    object: &mut Map<String, Value>,
    matches_state: &impl Fn(&[u8]) -> bool,
) -> bool {
    let mut changed = false;
    for field in TURN_STATE_FIELDS {
        if object.get(*field).is_some_and(|value| {
            !value
                .as_str()
                .is_some_and(|state| matches_state(state.as_bytes()))
        }) {
            object.remove(*field);
            changed = true;
        }
    }
    changed
}

fn scoped_turn_metadata(raw: &str, matches_state: &impl Fn(&[u8]) -> bool) -> Option<String> {
    if raw.len() > MAX_METADATA_BYTES {
        return None;
    }
    let mut metadata: Map<String, Value> = serde_json::from_str(raw).ok()?;
    let mut changed = remove_fields(&mut metadata, CLIENT_IDENTITY_FIELDS);
    changed |= remove_fields(&mut metadata, METADATA_RESPONSE_FIELDS);
    // Turn metadata contains client correlation data, not another metadata envelope.
    changed |= remove_fields(&mut metadata, TURN_METADATA_FIELDS);
    changed |= guard_state_fields(&mut metadata, matches_state);
    if metadata.is_empty() {
        return None;
    }
    if !changed && raw.is_ascii() {
        return Some(raw.to_string());
    }
    let json = serde_json::to_string(&metadata).ok()?;
    // Codex carries this JSON in HTTP headers and WS metadata. Keep its ASCII
    // representation, including UTF-16 surrogate pairs for non-BMP characters.
    let mut ascii = String::with_capacity(json.len());
    for character in json.chars() {
        if character.is_ascii() {
            ascii.push(character);
        } else {
            use std::fmt::Write;
            for unit in character.encode_utf16(&mut [0; 2]) {
                write!(ascii, "\\u{unit:04x}").ok()?;
            }
        }
    }
    (ascii.len() <= MAX_METADATA_BYTES).then_some(ascii)
}

fn remove_fields(object: &mut Map<String, Value>, fields: &[&str]) -> bool {
    let mut changed = false;
    for field in fields {
        changed |= object.remove(*field).is_some();
    }
    changed
}

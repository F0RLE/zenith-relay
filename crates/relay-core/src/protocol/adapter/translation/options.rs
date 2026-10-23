//! Non-generation request options. Native forwarding never normalizes these.
use super::{checked, AdapterError, AdapterResult, BTreeMap, Value, WireApi};
use sha2::{Digest, Sha256};

pub(super) fn metadata(
    request: &Value,
    protocol: WireApi,
) -> AdapterResult<BTreeMap<String, String>> {
    let Some(metadata) = request.get("metadata").filter(|value| !value.is_null()) else {
        return Ok(BTreeMap::new());
    };
    let invalid = || AdapterError::invalid_request().with_parameter("metadata");
    let fields = metadata.as_object().ok_or_else(invalid)?;
    if protocol == WireApi::Messages {
        checked(metadata, &["user_id"]).map_err(|error| error.with_parameter("metadata"))?;
    }
    if fields.len() > 16 {
        return Err(invalid());
    }
    let mut result = BTreeMap::new();
    for (key, value) in fields {
        // Messages explicitly permits a null user_id (equivalent to absent).
        if protocol == WireApi::Messages && value.is_null() {
            continue;
        }
        let value = value.as_str().ok_or_else(invalid)?;
        if key.chars().count() > 64 || value.chars().count() > 512 {
            return Err(invalid());
        }
        result.insert(key.clone(), value.into());
    }
    Ok(result)
}

pub(super) fn write_metadata(
    fields: &mut serde_json::Map<String, Value>,
    metadata: &BTreeMap<String, String>,
    protocol: WireApi,
    origin: Option<WireApi>,
) -> AdapterResult<()> {
    if metadata.is_empty() {
        return Ok(());
    }
    if protocol == WireApi::Gemini {
        if origin != Some(WireApi::Messages) || metadata.len() != 1 {
            return Err(AdapterError::parameter_unsupported_for("metadata"));
        }
        let user_id = metadata
            .get("user_id")
            .ok_or_else(|| AdapterError::parameter_unsupported_for("metadata"))?;
        // Gemini safety labels allow 63 lowercase characters; keep attribution opaque, never in content.
        let identifier = if user_id.is_empty() {
            String::new()
        } else {
            hex::encode(Sha256::digest(user_id.as_bytes()))[..63].to_owned()
        };
        fields.insert(
            "labels".into(),
            serde_json::json!({"safety_identifier": identifier}),
        );
        return Ok(());
    }
    if protocol == WireApi::Messages && metadata.keys().any(|key| key != "user_id") {
        return Err(AdapterError::parameter_unsupported_for("metadata"));
    }
    fields.insert("metadata".into(), serde_json::json!(metadata));
    Ok(())
}

/// Cache breakpoints are advisory provider optimizations, not conversation
/// content. Validate the Messages contract, then let the selected foreign
/// provider apply its own cache policy; never forward Anthropic TTLs as if
/// they were portable generation controls or promise equivalent billing.
pub(super) fn messages_fields(container: &Value, allowed: &[&str]) -> AdapterResult<()> {
    let mut fields = allowed.to_vec();
    fields.push("cache_control");
    checked(container, &fields)?;
    let Some(hint) = container
        .get("cache_control")
        .filter(|hint| !hint.is_null())
    else {
        return Ok(());
    };
    let invalid = || AdapterError::invalid_request().with_parameter("cache_control");
    checked(hint, &["type", "ttl"]).map_err(|error| error.with_parameter("cache_control"))?;
    if hint.get("type").and_then(Value::as_str) != Some("ephemeral") {
        return Err(invalid());
    }
    if hint
        .get("ttl")
        .is_some_and(|ttl| !matches!(ttl.as_str(), Some("5m" | "1h")))
    {
        return Err(invalid());
    }
    Ok(())
}

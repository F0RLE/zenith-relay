use crate::{runtime::DefaultServiceTier, WireApi};
use serde_json::{json, Map, Value};

/// Owns the service-tier field for one routed request.
///
/// Managed Codex requests must use the pool's two-value policy. Generic API
/// clients retain an explicit upstream tier such as `flex`. A request may be
/// retried on several candidates, so the policy removes stale state before
/// applying the selected candidate's pool setting each time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::gateway) struct ServiceTierPolicy {
    owner: ServiceTierOwner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceTierOwner {
    Client,
    Pool,
}

impl ServiceTierPolicy {
    pub(in crate::gateway) const fn client_owned() -> Self {
        Self {
            owner: ServiceTierOwner::Client,
        }
    }

    pub(in crate::gateway) const fn pool_owned() -> Self {
        Self {
            owner: ServiceTierOwner::Pool,
        }
    }

    pub(in crate::gateway) fn prepare_for_candidate(
        self,
        request: &mut Value,
        default: DefaultServiceTier,
        wire_api: WireApi,
    ) {
        let object = request
            .as_object_mut()
            .expect("request object was validated before routing");
        if self.owner == ServiceTierOwner::Pool {
            object.remove("service_tier");
            if wire_api != WireApi::Messages {
                apply_default_service_tier_if_missing(request, default);
            }
        }
    }

    pub(in crate::gateway) fn effective_tier(
        self,
        request: &Value,
        default: DefaultServiceTier,
        wire_api: WireApi,
    ) -> DefaultServiceTier {
        if self.owner == ServiceTierOwner::Pool && wire_api == WireApi::Messages {
            return default;
        }
        if wire_api != WireApi::Messages {
            request_service_tier(request)
        } else {
            DefaultServiceTier::Standard
        }
    }
}

pub(in crate::gateway) fn request_service_tier(request: &Value) -> DefaultServiceTier {
    if request
        .get("service_tier")
        .and_then(Value::as_str)
        .is_some_and(|tier| {
            tier.eq_ignore_ascii_case("priority") || tier.eq_ignore_ascii_case("fast")
        })
    {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

/// Apply the pool's Fast setting after the request owner has removed any tier
/// it does not control.
///
/// `priority` is the upstream OpenAI spelling. Standard deliberately remains
/// implicit, matching the Codex/Cockpit behavior and preserving arbitrary
/// client-owned values such as `flex`.
pub(in crate::gateway) fn apply_default_service_tier_if_missing(
    request: &mut Value,
    default: DefaultServiceTier,
) {
    if default != DefaultServiceTier::Fast {
        return;
    }
    let Some(object) = request.as_object_mut() else {
        return;
    };
    if object.contains_key("service_tier") {
        return;
    }
    object.insert(
        "service_tier".to_string(),
        Value::String("priority".to_string()),
    );
}

pub(in crate::gateway) fn normalize_account_request(
    object: &mut Map<String, Value>,
    responses_lite: bool,
) {
    // This transport normalization preserves native account settings. The
    // request execution layer applies Relay's two-speed pool policy later,
    // while Responses Lite alone requires `context=all_turns` here.
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    object.remove("max_output_tokens");
    sanitize_unstored_reasoning_items(object);
    if responses_lite {
        // Codex Responses Lite accepts only complete reasoning history. Keep
        // the client-selected effort and summary settings, but always supply
        // the mandatory context mode before the request reaches either the
        // HTTP or WebSocket account transport.
        let reasoning = object
            .entry("reasoning".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !reasoning.is_object() {
            *reasoning = Value::Object(Map::new());
        }
        reasoning
            .as_object_mut()
            .expect("reasoning was normalized to an object")
            .insert(
                "context".to_string(),
                Value::String("all_turns".to_string()),
            );
        // Responses Lite has a stricter tool contract than the regular
        // Responses endpoint.  The upstream currently requires an explicit
        // boolean and only supports serial tool execution, even when the
        // client omitted the field.  Keep the client-owned tool definitions
        // untouched, but pin this transport-level switch to false.  This is
        // deliberately done for both OAuth and compact Lite routes so HTTP
        // and WebSocket requests cannot diverge.
        if !matches!(object.get("parallel_tool_calls"), Some(Value::Bool(false))) {
            object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
        }
    }
    match object.get("input") {
        Some(Value::String(text)) if text.trim().is_empty() => {
            object.insert("input".to_string(), Value::Array(Vec::new()));
        }
        Some(Value::String(text)) => {
            object.insert(
                "input".to_string(),
                json!([{"role": "user", "content": [{"type": "input_text", "text": text}]}]),
            );
        }
        Some(Value::Object(item)) => {
            object.insert(
                "input".to_string(),
                Value::Array(vec![Value::Object(item.clone())]),
            );
        }
        _ => {}
    }
}

pub(in crate::gateway) fn responses_lite_parallel_tool_calls_valid(
    object: &Map<String, Value>,
) -> bool {
    object
        .get("parallel_tool_calls")
        .is_none_or(Value::is_boolean)
}

fn sanitize_unstored_reasoning_items(object: &mut Map<String, Value>) {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            continue;
        }
        let has_encrypted_content = item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if !has_encrypted_content {
            item.remove("id");
            item.remove("encrypted_content");
        }
    }
}

pub(in crate::gateway) fn try_recover_encrypted_content(
    request: &mut Value,
    attempted: &mut bool,
) -> bool {
    if *attempted {
        return false;
    }
    let mut recovered = request.clone();
    let mut changed = false;
    strip_encrypted_reasoning(&mut recovered, &mut changed);
    if !changed {
        return false;
    }
    *request = recovered;
    *attempted = true;
    true
}

fn strip_encrypted_reasoning(value: &mut Value, changed: &mut bool) {
    match value {
        Value::Array(values) => {
            values.retain_mut(|value| {
                if is_encrypted_compaction(value) {
                    *changed = true;
                    return false;
                }
                strip_encrypted_reasoning(value, changed);
                true
            });
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("reasoning")
                && object
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.trim().is_empty())
            {
                object.remove("encrypted_content");
                object.remove("id");
                *changed = true;
            }
            for value in object.values_mut() {
                strip_encrypted_reasoning(value, changed);
            }
        }
        _ => {}
    }
}

fn is_encrypted_compaction(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    matches!(
        object.get("type").and_then(Value::as_str),
        Some("compaction" | "compaction_summary")
    ) && object
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|content| !content.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pool_owned_service_tier_overrides_client_and_reapplies_for_each_candidate() {
        let policy = ServiceTierPolicy::pool_owned();
        let mut request = json!({"service_tier": "auto"});

        policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
        assert_eq!(request["service_tier"], "priority");

        policy.prepare_for_candidate(
            &mut request,
            DefaultServiceTier::Standard,
            WireApi::Responses,
        );
        assert!(request.get("service_tier").is_none());
        assert_eq!(
            policy.effective_tier(&request, DefaultServiceTier::Standard, WireApi::Responses),
            DefaultServiceTier::Standard
        );

        policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
        assert_eq!(request["service_tier"], "priority");
        assert_eq!(
            policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Responses),
            DefaultServiceTier::Fast
        );
    }

    #[test]
    fn client_owned_service_tier_preserves_explicit_value() {
        let mut request = json!({"service_tier": "flex"});
        let policy = ServiceTierPolicy::client_owned();

        policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
        assert_eq!(request["service_tier"], "flex");
        assert_eq!(
            policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Responses),
            DefaultServiceTier::Standard
        );

        let mut implicit = json!({});
        policy.prepare_for_candidate(&mut implicit, DefaultServiceTier::Fast, WireApi::Responses);
        assert!(implicit.get("service_tier").is_none());
    }

    #[test]
    fn pool_owned_service_tier_does_not_inject_into_messages() {
        let mut request = json!({"service_tier": "priority"});
        let policy = ServiceTierPolicy::pool_owned();

        policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Messages);
        assert!(request.get("service_tier").is_none());
        assert_eq!(
            policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Messages),
            DefaultServiceTier::Fast
        );
    }
}

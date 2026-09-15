use crate::{runtime::DefaultServiceTier, WireApi};
use serde_json::{json, Map, Number, Value};

const CODEX_TOOL_CONST_UNION_THRESHOLD: usize = 8;

/// Owns the service-tier field for one routed request.
///
/// Managed Codex requests use the pool's speed policy only when the
/// client did not select an upstream tier. Generic API clients retain their
/// explicit upstream tier such as `flex`. A request may be retried on several
/// candidates, so the original client field is retained separately from a
/// Relay-injected default and reapplied for every attempt.
#[derive(Clone, Debug)]
pub(in crate::gateway) struct ServiceTierPolicy {
    owner: ServiceTierOwner,
    client_tier: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceTierOwner {
    Client,
    Pool,
}

impl ServiceTierPolicy {
    pub(in crate::gateway) fn client_owned(request: &Value) -> Self {
        Self {
            owner: ServiceTierOwner::Client,
            client_tier: request
                .as_object()
                .and_then(|object| object.get("service_tier"))
                .cloned(),
        }
    }

    pub(in crate::gateway) fn pool_owned(request: &Value) -> Self {
        Self {
            owner: ServiceTierOwner::Pool,
            client_tier: request
                .as_object()
                .and_then(|object| object.get("service_tier"))
                .cloned(),
        }
    }

    pub(in crate::gateway) fn prepare_for_candidate(
        &self,
        request: &mut Value,
        default: DefaultServiceTier,
        wire_api: WireApi,
    ) {
        let object = request
            .as_object_mut()
            .expect("request object was validated before routing");
        // Remove a Relay-injected `priority` from the previous attempt first.
        // A fallback from Fast to Standard must not inherit that stale field.
        object.remove("service_tier");
        if let Some(client_tier) = self.client_tier.as_ref() {
            object.insert("service_tier".to_string(), client_tier.clone());
        } else if self.owner == ServiceTierOwner::Pool && wire_api != WireApi::Messages {
            apply_default_service_tier_if_missing(request, default);
        }
    }

    pub(in crate::gateway) fn effective_tier(
        &self,
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
    match request.get("service_tier").and_then(Value::as_str) {
        Some(tier) if tier.eq_ignore_ascii_case("ultrafast") => DefaultServiceTier::Ultrafast,
        Some(tier)
            if tier.eq_ignore_ascii_case("priority") || tier.eq_ignore_ascii_case("fast") =>
        {
            DefaultServiceTier::Fast
        }
        _ => DefaultServiceTier::Standard,
    }
}

/// Apply the pool's speed setting after the request owner has removed any tier
/// it does not control.
///
/// `priority` is the upstream OpenAI spelling for Fast. Standard deliberately
/// remains implicit, matching the Codex/Cockpit behavior and preserving
/// arbitrary client-owned values such as `flex`.
pub(in crate::gateway) fn apply_default_service_tier_if_missing(
    request: &mut Value,
    default: DefaultServiceTier,
) {
    let Some(object) = request.as_object_mut() else {
        return;
    };
    if object.contains_key("service_tier") {
        return;
    }
    let value = match default {
        DefaultServiceTier::Standard => return,
        DefaultServiceTier::Fast => "priority",
        DefaultServiceTier::Ultrafast => "ultrafast",
    };
    object.insert("service_tier".to_string(), Value::String(value.to_string()));
}

pub(in crate::gateway) fn normalize_account_request(
    object: &mut Map<String, Value>,
    responses_lite: bool,
) {
    // This transport normalization preserves native account settings. The
    // request execution layer applies Relay's pool speed policy later, while
    // Responses Lite alone requires `context=all_turns` here.
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    normalize_account_request_common(object, responses_lite);
}

/// Normalize the legacy non-streaming account compaction contract.
///
/// Unlike a regular Responses request, `/responses/compact` does not accept
/// the streaming transport controls that Relay adds for the normal account
/// path. Remove them even when a client supplied them explicitly; otherwise a
/// request can pass local validation and still be rejected by the account
/// endpoint. All other request fields remain client-owned so newly introduced
/// Codex options are not silently discarded.
pub(in crate::gateway) fn normalize_compact_account_request(
    object: &mut Map<String, Value>,
    responses_lite: bool,
) {
    object.remove("store");
    object.remove("stream");
    normalize_account_request_common(object, responses_lite);
}

fn normalize_account_request_common(object: &mut Map<String, Value>, responses_lite: bool) {
    object.remove("max_output_tokens");
    normalize_codex_tool_schemas(object);
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
        normalize_responses_lite_request(object);
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

/// Apply the transport-level Responses Lite tool contract.
///
/// The Lite marker can arrive on a WebSocket before Relay has selected a
/// concrete route. Normalize it at request parse time as a defensive guard so
/// a route that does not use the account normalizer cannot forward
/// `parallel_tool_calls: true` alongside the Lite contract.
pub(in crate::gateway) fn normalize_responses_lite_request(object: &mut Map<String, Value>) {
    if !matches!(object.get("parallel_tool_calls"), Some(Value::Bool(false))) {
        object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
    }
}

/// Codex rejects some large schemas emitted by MCP tools when an enum is
/// represented as a long `oneOf`/`anyOf` list of constant branches. Collapse
/// only branches that are provably equivalent to an enum. All other schema
/// shapes remain byte-for-byte equivalent at the JSON value level.
fn normalize_codex_tool_schemas(object: &mut Map<String, Value>) {
    let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    for tool in tools {
        normalize_codex_tool(tool);
    }
}

fn normalize_codex_tool(tool: &mut Value) {
    let Some(tool_object) = tool.as_object_mut() else {
        return;
    };
    match tool_object.get("type").and_then(Value::as_str) {
        Some("namespace") => {
            if let Some(nested_tools) = tool_object.get_mut("tools").and_then(Value::as_array_mut) {
                for nested_tool in nested_tools {
                    normalize_codex_tool(nested_tool);
                }
            }
        }
        Some("function" | "custom") => {
            if let Some(parameters) = tool_object.get_mut("parameters") {
                normalize_codex_schema(parameters);
            }
        }
        _ => {}
    }
}

fn normalize_codex_schema(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };

    // Visit nested object/array schemas before the current node. This covers
    // MCP schemas nested below properties/items without changing unrelated
    // tool metadata or choice constraints.
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for property in properties.values_mut() {
            normalize_codex_schema(property);
        }
    }
    if let Some(items) = object.get_mut("items") {
        normalize_codex_schema(items);
    }

    let union_name = match (object.contains_key("oneOf"), object.contains_key("anyOf")) {
        (true, true) | (false, false) => return,
        (true, false) => "oneOf",
        (false, true) => "anyOf",
    };
    let Some(union) = object.get(union_name).and_then(Value::as_array) else {
        return;
    };
    if union.len() < CODEX_TOOL_CONST_UNION_THRESHOLD {
        return;
    }

    let mut branches = Vec::with_capacity(union.len());
    let mut semantic_keys = Vec::with_capacity(union.len());
    for branch in union {
        let Some(branch_object) = branch.as_object() else {
            return;
        };
        let Some(const_value) = branch_object.get("const") else {
            return;
        };
        if branch_object
            .keys()
            .any(|key| !matches!(key.as_str(), "const" | "description" | "title"))
        {
            return;
        }
        let Some(key) = canonical_codex_scalar_key(const_value) else {
            return;
        };
        if semantic_keys.iter().any(|seen| seen == &key) {
            return;
        }
        semantic_keys.push(key);
        branches.push(const_value.clone());
    }

    if let Some(existing_enum) = object.get("enum").and_then(Value::as_array) {
        let Some(existing_keys) = existing_enum
            .iter()
            .map(canonical_codex_scalar_key)
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if !same_codex_scalar_set(&existing_keys, &semantic_keys) {
            return;
        }
        object.remove(union_name);
        return;
    }

    object.insert("enum".to_string(), Value::Array(branches));
    object.remove(union_name);
}

fn canonical_codex_scalar_key(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(format!("s:{value}")),
        Value::Number(value) => Some(format!("n:{}", canonical_codex_number(value))),
        Value::Bool(value) => Some(format!("b:{value}")),
        Value::Null => Some("null".to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn canonical_codex_number(value: &Number) -> String {
    let raw = value.to_string();
    let (mantissa, exponent) = raw
        .split_once(['e', 'E'])
        .map_or((raw.as_str(), 0_i64), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i64>().unwrap_or(0))
        });
    let (sign, unsigned) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |unsigned| ("-", unsigned));
    let (whole, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, ""), |parts| parts);
    let mut digits = format!("{whole}{fraction}");
    let first_non_zero = digits.find(|digit| digit != '0');
    let Some(first_non_zero) = first_non_zero else {
        return "0".to_string();
    };
    digits.drain(..first_non_zero);
    let mut scale = exponent - i64::try_from(fraction.len()).unwrap_or(i64::MAX);
    while digits.ends_with('0') {
        digits.pop();
        scale = scale.saturating_add(1);
    }
    format!("{sign}{digits}e{scale}")
}

fn same_codex_scalar_set(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left.len() == left.iter().collect::<std::collections::HashSet<_>>().len()
        && left.iter().all(|value| right.contains(value))
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
    fn pool_default_applies_only_when_client_did_not_select_a_tier() {
        let mut request = json!({});
        let policy = ServiceTierPolicy::pool_owned(&request);

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

        let mut explicit = json!({"service_tier": "flex"});
        let explicit_policy = ServiceTierPolicy::pool_owned(&explicit);
        explicit_policy.prepare_for_candidate(
            &mut explicit,
            DefaultServiceTier::Fast,
            WireApi::Responses,
        );
        assert_eq!(explicit["service_tier"], "flex");
        assert_eq!(
            explicit_policy.effective_tier(&explicit, DefaultServiceTier::Fast, WireApi::Responses),
            DefaultServiceTier::Standard
        );

        explicit_policy.prepare_for_candidate(
            &mut explicit,
            DefaultServiceTier::Standard,
            WireApi::Responses,
        );
        assert_eq!(explicit["service_tier"], "flex");
    }

    #[test]
    fn client_owned_service_tier_preserves_explicit_value() {
        let mut request = json!({"service_tier": "flex"});
        let policy = ServiceTierPolicy::client_owned(&request);

        policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
        assert_eq!(request["service_tier"], "flex");
        assert_eq!(
            policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Responses),
            DefaultServiceTier::Standard
        );

        let mut implicit = json!({});
        let implicit_policy = ServiceTierPolicy::client_owned(&implicit);
        implicit_policy.prepare_for_candidate(
            &mut implicit,
            DefaultServiceTier::Fast,
            WireApi::Responses,
        );
        assert!(implicit.get("service_tier").is_none());
    }

    #[test]
    fn pool_owned_service_tier_does_not_inject_into_messages() {
        let mut request = json!({"service_tier": "priority"});
        let policy = ServiceTierPolicy::pool_owned(&request);

        policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Messages);
        assert_eq!(request["service_tier"], "priority");
        assert_eq!(
            policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Messages),
            DefaultServiceTier::Fast
        );
    }

    #[test]
    fn pool_owned_service_tier_injects_ultrafast_and_tracks_it() {
        let mut request = json!({});
        let policy = ServiceTierPolicy::pool_owned(&request);

        policy.prepare_for_candidate(
            &mut request,
            DefaultServiceTier::Ultrafast,
            WireApi::Responses,
        );
        assert_eq!(request["service_tier"], "ultrafast");
        assert_eq!(
            policy.effective_tier(&request, DefaultServiceTier::Ultrafast, WireApi::Responses),
            DefaultServiceTier::Ultrafast
        );

        policy.prepare_for_candidate(
            &mut request,
            DefaultServiceTier::Standard,
            WireApi::Responses,
        );
        assert!(request.get("service_tier").is_none());
    }

    fn const_branches(values: &[Value]) -> Value {
        Value::Array(
            values
                .iter()
                .map(|value| json!({"const": value, "description": "choice"}))
                .collect(),
        )
    }

    #[test]
    fn codex_tool_schema_normalization_flattens_large_pure_unions() {
        let values: Vec<Value> = (0..8).map(|value| json!(value)).collect();
        let mut request = json!({
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {"oneOf": const_branches(&values)}
                    }
                }
            }]
        });

        normalize_account_request(request.as_object_mut().unwrap(), false);

        assert_eq!(
            request["tools"][0]["parameters"]["properties"]["kind"]["enum"],
            Value::Array(values)
        );
        assert!(request["tools"][0]["parameters"]["properties"]["kind"]
            .get("oneOf")
            .is_none());
    }

    #[test]
    fn compact_normalization_removes_transport_fields_and_preserves_new_fields() {
        let mut request = json!({
            "store": false,
            "stream": false,
            "max_output_tokens": 4,
            "model": "gpt-test",
            "input": "compact this",
            "reasoning": {"effort": "high"},
            "future_compaction_option": {"enabled": true}
        });

        normalize_compact_account_request(request.as_object_mut().unwrap(), false);

        assert!(request.get("store").is_none());
        assert!(request.get("stream").is_none());
        assert!(request.get("max_output_tokens").is_none());
        assert_eq!(request["reasoning"]["effort"], "high");
        assert_eq!(request["future_compaction_option"]["enabled"], true);
        assert_eq!(request["input"][0]["content"][0]["text"], "compact this");
    }

    #[test]
    fn codex_tool_schema_normalization_reaches_nested_namespace_tools() {
        let values: Vec<Value> = (0..8)
            .map(|value| json!(format!("choice-{value}")))
            .collect();
        let mut request = json!({
            "tools": [{
                "type": "namespace",
                "tools": [{
                    "type": "custom",
                    "name": "nested",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "kind": {"anyOf": const_branches(&values)}
                        }
                    }
                }]
            }]
        });

        normalize_account_request(request.as_object_mut().unwrap(), false);

        assert_eq!(
            request["tools"][0]["tools"][0]["parameters"]["properties"]["kind"]["enum"],
            Value::Array(values)
        );
    }

    #[test]
    fn codex_tool_schema_normalization_preserves_non_pure_or_duplicate_unions() {
        let values: Vec<Value> = (0..8).map(|value| json!(value)).collect();
        let mut request = json!({
            "tools": [{
                "type": "function",
                "parameters": {
                    "properties": {
                        "constrained": {"oneOf": [
                            {"const": "a"}, {"const": "b", "type": "string"},
                            {"const": "c"}, {"const": "d"}, {"const": "e"},
                            {"const": "f"}, {"const": "g"}, {"const": "h"}
                        ]},
                        "duplicate": {"oneOf": const_branches(&[
                            values[0].clone(), values[1].clone(), values[2].clone(), values[3].clone(),
                            values[4].clone(), values[5].clone(), values[6].clone(), values[6].clone()
                        ])}
                    }
                }
            }]
        });
        let original = request["tools"].clone();

        normalize_account_request(request.as_object_mut().unwrap(), false);

        assert_eq!(request["tools"], original);
    }

    #[test]
    fn codex_tool_schema_normalization_treats_equivalent_numbers_as_duplicates() {
        let mut request = json!({
            "tools": [{
                "type": "function",
                "parameters": {
                    "properties": {
                        "kind": {"oneOf": [
                            {"const": 1}, {"const": 1.0}, {"const": 2}, {"const": 3},
                            {"const": 4}, {"const": 5}, {"const": 6}, {"const": 7}
                        ]}
                    }
                }
            }]
        });

        normalize_account_request(request.as_object_mut().unwrap(), false);

        assert!(request["tools"][0]["parameters"]["properties"]["kind"]
            .get("enum")
            .is_none());
        assert!(request["tools"][0]["parameters"]["properties"]["kind"]
            .get("oneOf")
            .is_some());
    }
}

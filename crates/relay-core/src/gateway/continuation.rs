use crate::error_codes;
mod compacted;

use super::request::{tool_call_output_ids, unpaired_tool_output_ids};
use crate::GatewayRuntime;
use serde_json::{Map, Value};

pub(super) const RESPONSE_CONTINUATION_UNAVAILABLE_CODE: &str =
    error_codes::RESPONSE_CONTINUATION_UNAVAILABLE;
pub(super) const RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE: &str =
    "response continuation is unavailable; resend complete history without previous_response_id or start a new conversation";

/// Shared ownership facts for HTTP, WebSocket, and account-only execution.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct ContinuationState {
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) has_unpaired_tool_output: bool,
}

/// Classifies an incoming Responses continuation before candidate selection.
///
/// An opaque ID created outside this Relay instance has no known owner. It
/// must never be forwarded to an arbitrary candidate. The shape of incoming
/// messages cannot establish that the client included all earlier context.
pub(super) fn prepare_response_continuation(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    now_ms: u64,
    connection_affinity_key: Option<&str>,
) -> Result<ContinuationState, ()> {
    if compacted::reset_compacted_history(request) {
        return Ok(ContinuationState {
            response_affinity_key: None,
            requires_affinity_owner: false,
            has_unpaired_tool_output: false,
        });
    }
    let response_affinity_key = connection_affinity_key.map(str::to_string).or_else(|| {
        runtime.response_affinity_key(request.get("previous_response_id").and_then(Value::as_str))
    });
    let response_binding_known = response_affinity_key
        .as_deref()
        .is_some_and(|key| runtime.has_response_affinity_binding(key, now_ms));
    let tool_output_ids = tool_call_output_ids(request);
    let unpaired_output_ids = unpaired_tool_output_ids(request);
    let has_unpaired_tool_output = !unpaired_output_ids.is_empty();
    let tool_affinity_key = if !response_binding_known && has_unpaired_tool_output {
        let mut owner = None;
        let mut owner_key = None;
        // Completed historic calls do not establish ownership of new results.
        // Every unpaired output must resolve to the same known candidate.
        for call_id in unpaired_output_ids {
            let affinity_key = runtime
                .tool_call_affinity_key(local_key_id, &call_id)
                .ok_or(())?;
            let candidate = runtime
                .response_affinity_candidate(&affinity_key, now_ms)
                .ok_or(())?;
            if owner.as_ref().is_some_and(|owner| owner != &candidate) {
                return Err(());
            }
            owner = Some(candidate);
            owner_key = Some(affinity_key);
        }
        owner_key
    } else {
        tool_output_ids.iter().find_map(|call_id| {
            let affinity_key = runtime.tool_call_affinity_key(local_key_id, call_id)?;
            runtime
                .has_response_affinity_binding(&affinity_key, now_ms)
                .then_some(affinity_key)
        })
    };
    let has_previous_response_id = response_affinity_key.is_some();
    if has_previous_response_id && !response_binding_known {
        return Err(());
    }
    if !response_binding_known && has_unpaired_tool_output && tool_affinity_key.is_none() {
        return Err(());
    }

    let response_affinity_key = if response_binding_known {
        response_affinity_key
    } else {
        tool_affinity_key.or(response_affinity_key)
    };
    Ok(ContinuationState {
        // A complete tool call plus its matching output is self-contained:
        // it can be sent to another compatible candidate without an opaque
        // response reference. Only an actual previous response or an
        // unpaired tool output still needs its creating owner.
        requires_affinity_owner: has_previous_response_id || has_unpaired_tool_output,
        response_affinity_key,
        has_unpaired_tool_output,
    })
}

/// Materializes the saved predecessor before releasing its owner. Plaintext
/// replay can change models; tool and encrypted state use the stricter native
/// replay path. Missing, evicted or differently scoped state stays owner-bound.
pub(super) fn drop_materialized_previous_response_id(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    model: &str,
    now_ms: u64,
) -> bool {
    if compacted::reset_compacted_history(request) {
        return true;
    }
    let Some(previous_id) = request.get("previous_response_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(owner) = runtime
        .response_affinity_key(Some(previous_id))
        .and_then(|key| runtime.response_affinity_candidate(&key, now_ms))
    else {
        return false;
    };
    let Some(replay) =
        runtime.load_native_responses_replay(local_key_id, previous_id, &owner, now_ms)
    else {
        return false;
    };
    let Ok(mut materialized) = replay.replay_request(
        request,
        replay.model(),
        request
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    ) else {
        return false;
    };
    if !has_materialized_plaintext_history(&materialized)
        || request
            .get("conversation")
            .is_some_and(|value| !value.is_null())
        || request.get("context_management").is_some()
    {
        return false;
    }
    materialized["model"] = Value::String(model.to_string());
    *request = materialized;
    true
}

fn has_materialized_plaintext_history(request: &Value) -> bool {
    if request.get("context_management").is_some()
        || request.get("truncation").is_some()
        || contains_encrypted_content(request)
        || contains_tool_state(request)
        || request
            .get("conversation")
            .is_some_and(|value| !value.is_null())
    {
        return false;
    }
    let Some(input) = request.get("input").and_then(Value::as_array) else {
        return false;
    };
    if input.is_empty() {
        return false;
    }

    for item in input {
        let Some(message) = item.as_object() else {
            return false;
        };
        if message
            .get("type")
            .is_some_and(|kind| kind.as_str() != Some("message"))
        {
            return false;
        }
        let Some(role) = message.get("role").and_then(Value::as_str) else {
            return false;
        };
        if !matches!(role, "user" | "assistant" | "developer" | "system")
            || !message_has_plaintext_content(message)
        {
            return false;
        }
    }
    true
}

fn message_has_plaintext_content(message: &Map<String, Value>) -> bool {
    match message.get("content") {
        Some(Value::String(_)) => true,
        Some(Value::Array(parts)) => !parts.is_empty() && parts.iter().all(plaintext_content_part),
        _ => false,
    }
}

fn plaintext_content_part(part: &Value) -> bool {
    let Some(part) = part.as_object() else {
        return false;
    };
    match part.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "text") => {
            part.get("text").is_some_and(Value::is_string)
        }
        Some("refusal") => part.get("refusal").is_some_and(Value::is_string),
        _ => false,
    }
}

fn contains_encrypted_content(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_encrypted_content),
        Value::Object(values) => {
            values.contains_key("encrypted_content")
                || values.values().any(contains_encrypted_content)
        }
        _ => false,
    }
}

fn contains_tool_state(request: &Value) -> bool {
    request
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(is_tool_state_item))
}

fn is_tool_state_item(item: &Value) -> bool {
    let Some(item) = item.as_object() else {
        return false;
    };
    let kind = item.get("type").and_then(Value::as_str);
    kind.is_some_and(|kind| {
        kind.ends_with("_call") || kind.ends_with("_call_output") || kind.ends_with("_output")
    }) || matches!(
        item.get("role").and_then(Value::as_str),
        Some("tool" | "function")
    ) || ["tool_calls", "tool_call_id", "function_call"]
        .iter()
        .any(|field| item.contains_key(*field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource,
        WireApi,
    };
    use serde_json::json;
    use std::sync::Arc;

    fn runtime() -> GatewayRuntime {
        GatewayRuntime::from_pool(
            ["source", "other-source"]
                .into_iter()
                .map(|id| {
                    RuntimeSource::unrestricted(ProviderSource {
                        id: id.into(),
                        name: "Test".into(),
                        base_url: "http://127.0.0.1:9/v1".into(),
                        api_key: "synthetic".into(),
                        wire_api: WireApi::Responses,
                        models: vec!["model".into()],
                    })
                })
                .collect(),
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "synthetic-key".into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap()
    }

    #[test]
    fn client_history_never_proves_an_opaque_predecessor_is_complete() {
        let runtime = runtime();
        for input in [
            json!([{"role":"assistant","content":"Acknowledged"},{"role":"user","content":"Apply the earlier constraints"}]),
            json!([{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi"},{"role":"user","content":"Continue"}]),
        ] {
            let mut request = json!({"previous_response_id":"resp_unknown","input":input});
            let original = request.clone();
            assert!(
                prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err()
            );
            assert!(!drop_materialized_previous_response_id(
                &runtime,
                "key",
                &mut request,
                "model",
                10
            ));
            assert_eq!(request, original);
            runtime.bind_response_affinity(Some("resp_unknown"), "source", 10);
            assert!(!drop_materialized_previous_response_id(
                &runtime,
                "key",
                &mut request,
                "model",
                10
            ));
            assert_eq!(request, original);
            runtime.invalidate_response_affinity(
                runtime
                    .response_affinity_key(Some("resp_unknown"))
                    .as_deref(),
            );
        }
    }

    #[test]
    fn a_paired_historic_call_cannot_own_an_unknown_tool_output() {
        let runtime = runtime();
        runtime.bind_tool_call_affinity("key", "paired", "source", 10);
        let mut request = json!({"input":[
            {"type":"function_call", "call_id":"paired", "name":"lookup", "arguments":"{}"},
            {"type":"function_call_output", "call_id":"paired", "output":"old result"},
            {"type":"function_call_output", "call_id":"unknown", "output":"new result"}
        ]});
        assert!(prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err());
    }

    #[test]
    fn unpaired_outputs_require_their_own_shared_owner() {
        let runtime = runtime();
        runtime.bind_tool_call_affinity("key", "historic", "source", 10);
        runtime.bind_tool_call_affinity("key", "current", "other-source", 10);
        runtime.bind_tool_call_affinity("key", "conflicting", "source", 10);
        let mut request = json!({"input":[
            {"type":"function_call", "call_id":"historic", "name":"lookup", "arguments":"{}"},
            {"type":"function_call_output", "call_id":"historic", "output":"old result"},
            {"type":"function_call_output", "call_id":"current", "output":"new result"}
        ]});
        let state = prepare_response_continuation(&runtime, "key", &mut request, 10, None).unwrap();
        assert!(state.requires_affinity_owner);
        assert_eq!(
            runtime
                .response_affinity_candidate(state.response_affinity_key.as_deref().unwrap(), 10)
                .as_deref(),
            Some("other-source")
        );
        request["input"].as_array_mut().unwrap().push(json!({
            "type":"function_call_output", "call_id":"conflicting", "output":"another result"
        }));
        assert!(prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err());
    }

    #[test]
    fn tool_schema_examples_do_not_establish_or_require_ownership() {
        let runtime = runtime();
        let mut request = json!({
            "tools":[{"type":"function","name":"inspect","parameters":{
                "type":"object","examples":[{
                    "type":"function_call", "call_id":"unknown", "name":"lookup", "arguments":"{}"
                }]
            }}],
            "input":[{"type":"function_call_output", "call_id":"unknown", "output":"new result"}]
        });
        assert!(prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err());

        request["tools"][0]["parameters"]["examples"] = request["input"].clone();
        request["input"] = json!([{"role":"user", "content":"Inspect the provided example"}]);
        let state = prepare_response_continuation(&runtime, "key", &mut request, 10, None).unwrap();
        assert!(!state.requires_affinity_owner);
    }

    #[test]
    fn ownership_validation_checks_tool_outputs_beyond_sixteen_items() {
        let runtime = runtime();
        let mut input = Vec::new();
        for index in 0..17 {
            input.push(json!({"type":"function_call", "call_id":format!("call_{index}"), "name":"lookup", "arguments":"{}"}));
            input.push(json!({"type":"function_call_output", "call_id":format!("call_{index}"), "output":"value"}));
        }
        let mut complete = json!({"input":input});
        assert!(
            !prepare_response_continuation(&runtime, "key", &mut complete, 10, None)
                .unwrap()
                .requires_affinity_owner
        );
        input.push(json!({"type":"function_call_output", "call_id":"unknown", "output":"value"}));
        let mut incomplete = json!({"input":input});
        assert!(prepare_response_continuation(&runtime, "key", &mut incomplete, 10, None).is_err());
    }

    #[test]
    fn saved_plaintext_chain_preserves_early_context_during_model_switch() {
        let runtime = runtime();
        let first = json!({"model":"model","input":"Remember the initial constraint"});
        let output = json!({"id":"resp_saved","output":[{"type":"message","role":"assistant","content":"Acknowledged"}]});
        runtime.capture_native_responses_replay("key", "source", &first, "model", &output, 10);
        runtime.bind_response_affinity(Some("resp_saved"), "source", 10);
        let mut request =
            json!({"previous_response_id":"resp_saved","input":"Continue","model":"new-model"});
        let original = request.clone();
        assert!(!drop_materialized_previous_response_id(
            &runtime,
            "other-key",
            &mut request,
            "new-model",
            10
        ));
        assert_eq!(request, original);
        assert!(drop_materialized_previous_response_id(
            &runtime,
            "key",
            &mut request,
            "new-model",
            10
        ));
        assert_eq!(request["model"], "new-model");
        assert!(request.get("previous_response_id").is_none());
        assert_eq!(
            request["input"][0]["content"][0]["text"],
            "Remember the initial constraint"
        );
        assert_eq!(request["input"][1], output["output"][0]);
        assert_eq!(request["input"][2]["content"][0]["text"], "Continue");
    }

    #[test]
    fn plaintext_recovery_rejects_saved_tools_and_encrypted_state() {
        for output in [
            json!([{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"}]),
            json!([{"type":"reasoning","encrypted_content":"synthetic","summary":[]}]),
        ] {
            let runtime = runtime();
            runtime.capture_native_responses_replay(
                "key",
                "source",
                &json!({"input":"start"}),
                "model",
                &json!({"id":"resp_saved","output":output}),
                10,
            );
            runtime.bind_response_affinity(Some("resp_saved"), "source", 10);
            let mut request = json!({"previous_response_id":"resp_saved","input":"continue"});
            let original = request.clone();
            assert!(!drop_materialized_previous_response_id(
                &runtime,
                "key",
                &mut request,
                "new-model",
                10
            ));
            assert_eq!(request, original);
        }
    }
}

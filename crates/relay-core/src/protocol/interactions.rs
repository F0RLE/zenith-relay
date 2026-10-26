//! Native Gemini Interactions model requests. Contract reference:
//! googleapis/python-genai, google/genai/_gaos/types/interactions (2026-10-24).
//! Generate Content support does not imply Interactions support. No conversion,
//! background agents, remote retrieval or automatic regeneration is provided.
use super::completion::ResponseCompletion;
use serde_json::Value;

pub(crate) fn reference(local_key: &str, id: &str) -> Result<String, ()> {
    crate::json_fingerprint::sha256(&("gemini-interactions", local_key, id))
        .map(|digest| format!("interactions:{}", hex::encode(digest)))
        .map_err(|_| ())
}

pub(crate) fn validate_request(request: &Value) -> Result<(), &'static str> {
    let fields = request
        .as_object()
        .ok_or("interactions request must be an object")?;
    if fields.contains_key("agent") || fields.get("background") == Some(&Value::Bool(true)) {
        return Err("background and agent interactions are not supported");
    }
    if fields
        .get("background")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("background must be a boolean");
    }
    if fields
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("interactions require a model");
    }
    if fields
        .get("input")
        .is_some_and(|value| !value.is_string() && !value.is_array())
    {
        return Err("interactions input must be text or an array");
    }
    previous_id(request).map(|_| ())
}

pub(crate) fn previous_id(request: &Value) -> Result<Option<&str>, &'static str> {
    match request.get("previous_interaction_id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id))
            if !id.is_empty()
                && id.len() <= 4096
                && !id.chars().any(char::is_control)
                && id.trim() == id =>
        {
            Ok(Some(id))
        }
        _ => Err("previous_interaction_id must be a nonempty opaque identifier"),
    }
}

pub(crate) fn resource(response: &Value) -> &Value {
    response.get("interaction").unwrap_or(response)
}

pub(crate) fn completion(response: &Value) -> ResponseCompletion {
    let interaction = resource(response);
    if !interaction.is_object()
        || interaction
            .get("error")
            .is_some_and(|error| !error.is_null())
        || interaction.get("errors").is_some_and(|errors| {
            !errors.is_null() && !errors.as_array().is_some_and(Vec::is_empty)
        })
    {
        return ResponseCompletion::Invalid;
    }
    let streamed = matches!(
        response.get("event_type").and_then(Value::as_str),
        Some("interaction.completed" | "interaction.status_update")
    );
    match interaction.get("status").and_then(Value::as_str) {
        Some("requires_action")
            if interaction
                .get("outputs")
                .is_some_and(|outputs| !valid_tool_outputs(outputs)) =>
        {
            ResponseCompletion::Invalid
        }
        Some("completed" | "requires_action")
            if streamed || interaction.get("outputs").is_some_and(Value::is_array) =>
        {
            ResponseCompletion::Completed
        }
        Some("cancelled" | "incomplete") => ResponseCompletion::Incomplete,
        _ => ResponseCompletion::Invalid,
    }
}

fn valid_tool_outputs(outputs: &Value) -> bool {
    outputs
        .as_array()
        .is_some_and(|items| items.iter().any(valid_function_call))
}

fn valid_function_call(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("function_call")
        && ["id", "name"].iter().all(|field| {
            item.get(*field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
}

#[derive(Default)]
pub(crate) struct InteractionsStreamCompletion {
    saw_function_call: bool,
}

impl InteractionsStreamCompletion {
    pub(crate) fn observe(&mut self, event: &Value) -> Option<ResponseCompletion> {
        if event.get("event_type").and_then(Value::as_str) == Some("content.start")
            && let Some(content) = event.get("content")
        {
            self.saw_function_call |= valid_function_call(content);
        }
        terminal_event(event).map(|completion| {
            if completion == ResponseCompletion::Completed
                && resource(event).get("status").and_then(Value::as_str) == Some("requires_action")
                && !self.saw_function_call
                && !resource(event)
                    .get("outputs")
                    .is_some_and(valid_tool_outputs)
            {
                ResponseCompletion::Invalid
            } else {
                completion
            }
        })
    }
}

pub(crate) fn terminal_event(event: &Value) -> Option<ResponseCompletion> {
    match event.get("event_type").and_then(Value::as_str) {
        Some("interaction.completed") => Some(completion(event)),
        Some("error") => Some(ResponseCompletion::Invalid),
        Some("interaction.status_update")
            if matches!(
                resource(event).get("status").and_then(Value::as_str),
                Some("failed" | "cancelled" | "requires_action")
            ) =>
        {
            Some(completion(event))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_requests_retain_tools_signatures_and_opaque_predecessors() {
        let body = json!({"model":"synthetic-model","previous_interaction_id":"opaque-id","input":[{"type":"function_result","call_id":"call-one","result":"ok"}],"tools":[{"type":"function","name":"lookup"}],"store":false});
        let before = body.clone();
        assert!(validate_request(&body).is_ok());
        assert_eq!(previous_id(&body), Ok(Some("opaque-id")));
        assert_eq!(body, before);
        for invalid in [
            json!({"model":"test","background":true}),
            json!({"agent":"test"}),
            json!({"model":"test","previous_interaction_id":[]}),
            json!({"model":"test","input":{}}),
        ] {
            assert!(validate_request(&invalid).is_err());
        }
    }

    #[test]
    fn lifecycle_events_do_not_fabricate_success_or_require_terminal_outputs() {
        assert_eq!(
            terminal_event(
                &json!({"event_type":"interaction.created","interaction":{"id":"i","status":"in_progress"}})
            ),
            None
        );
        assert_eq!(
            terminal_event(
                &json!({"event_type":"interaction.completed","interaction":{"id":"i","status":"completed","usage":{"total_input_tokens":3}}})
            ),
            Some(ResponseCompletion::Completed)
        );
        assert_eq!(
            completion(&json!({"status":"in_progress","outputs":[]})),
            ResponseCompletion::Invalid
        );
        assert_eq!(
            completion(&json!({"status":"completed","outputs":[],"errors":[{"code":"synthetic"}]})),
            ResponseCompletion::Invalid
        );
        assert_eq!(
            completion(
                &json!({"status":"requires_action","outputs":[{"type":"function_call","id":"c","name":"lookup"}]})
            ),
            ResponseCompletion::Completed
        );
    }
}

use super::contracts::{
    bridged_namespace_tool_name, custom_tool_item_id, prepare_bridge_state, request_tool_catalog,
    AdapterError, AdapterResult, ClientToolTarget, MessagesBridgeState, MessagesReasoningMode,
    ResponsesToolKind,
};
use serde_json::{json, Map, Value};

/// Validate the Gemini parts this adapter can preserve. Native routes do not
/// use this restriction and retain provider extensions without conversion.
pub(in crate::protocol::adapter) fn validate_response_part(
    part: &Value,
) -> AdapterResult<&Map<String, Value>> {
    let invalid = AdapterError::upstream_response_invalid;
    super::contracts::validate_bridge_fields(
        part,
        &["text", "thought", "thoughtSignature", "functionCall"],
    )
    .map_err(|_| invalid())?;
    let fields = part.as_object().ok_or_else(invalid)?;
    if fields.get("text").is_some_and(|value| !value.is_string())
        || fields
            .get("thought")
            .is_some_and(|value| !value.is_boolean())
        || fields
            .get("thoughtSignature")
            .is_some_and(|value| !value.is_string())
        || (fields.contains_key("text") && fields.contains_key("functionCall"))
        || !["text", "functionCall", "thoughtSignature"]
            .iter()
            .any(|field| fields.contains_key(*field))
    {
        return Err(invalid());
    }
    if let Some(call) = fields.get("functionCall") {
        super::contracts::validate_bridge_fields(
            call,
            &["id", "name", "args", "partialArgs", "willContinue"],
        )
        .map_err(|_| invalid())?;
        for field in ["id", "name"] {
            if call
                .get(field)
                .is_some_and(|value| value.as_str().is_none_or(|text| text.trim().is_empty()))
            {
                return Err(invalid());
            }
        }
        if call
            .get("willContinue")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(invalid());
        }
    }
    Ok(fields)
}

/// Complete Responses-to-Gemini request plus the local state needed by the
/// next `previous_response_id` turn.
#[derive(Clone, Debug)]
pub struct GeminiBridgeRequest {
    pub(super) upstream_body: Value,
    pub(super) model: String,
    pub(super) response_id: String,
    pub(super) bridge_state: MessagesBridgeState,
}

impl GeminiBridgeRequest {
    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn response_id(&self) -> &str {
        &self.response_id
    }
    pub(super) fn bridge_state(&self) -> &MessagesBridgeState {
        &self.bridge_state
    }
}

#[derive(Clone, Debug)]
pub struct GeminiBridgeResponse {
    pub response_body: Value,
    pub response_id: String,
    pub continuation: MessagesBridgeState,
}

/// Shared assembly for Gemini's complete calls and Vertex's argument fragments.
/// Client IDs belong to the output adapter; this state retains provider IDs.
#[derive(Debug, Default)]
pub(in crate::protocol::adapter) struct GeminiFunctionCalls {
    calls: Vec<GeminiFunctionCall>,
}

#[derive(Debug)]
pub(in crate::protocol::adapter) struct GeminiFunctionCall {
    pub(in crate::protocol::adapter) provider_id: Option<String>,
    pub(in crate::protocol::adapter) name: String,
    pub(in crate::protocol::adapter) arguments: Value,
    signature: Option<String>,
    argument_state: json_path::GeminiArgumentState,
    closed: bool,
}

impl GeminiFunctionCalls {
    pub(in crate::protocol::adapter) fn push(
        &mut self,
        part: &Value,
    ) -> AdapterResult<(usize, bool)> {
        let invalid = AdapterError::upstream_stream_invalid;
        let fields = validate_response_part(part).map_err(|_| invalid())?;
        let call = fields
            .get("functionCall")
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        let provider_id = call.get("id").and_then(Value::as_str);
        let name = call.get("name").and_then(Value::as_str);
        let exact = provider_id.and_then(|id| {
            self.calls
                .iter()
                .position(|call| call.provider_id.as_deref() == Some(id))
        });
        let existing = if exact.is_some() {
            exact
        } else {
            let mut possible = self.calls.iter().enumerate().filter(|(_, call)| {
                !call.closed
                    && (name.is_none() || Some(call.name.as_str()) == name)
                    && (provider_id.is_none() || call.provider_id.is_none())
            });
            let first = possible.next().map(|(index, _)| index);
            if possible.next().is_some() {
                return Err(invalid());
            }
            first
        };
        let is_new = existing.is_none();
        let index = if let Some(index) = existing {
            index
        } else {
            let name = name.ok_or_else(invalid)?;
            self.calls.push(GeminiFunctionCall {
                provider_id: None,
                name: name.into(),
                arguments: json!({}),
                signature: None,
                argument_state: json_path::GeminiArgumentState::default(),
                closed: false,
            });
            self.calls.len() - 1
        };
        let state = &mut self.calls[index];
        if state.closed
            || name.is_some_and(|name| name != state.name)
            || provider_id.is_some_and(|id| {
                state
                    .provider_id
                    .as_deref()
                    .is_some_and(|previous| id != previous)
            })
        {
            return Err(invalid());
        }
        if let Some(id) = provider_id {
            state.provider_id = Some(id.into());
        }
        state.arguments =
            json_path::merge_function_call_args(&state.arguments, call, &mut state.argument_state)
                .map_err(|_| invalid())?;
        if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
            if state
                .signature
                .as_deref()
                .is_some_and(|previous| previous != signature)
            {
                return Err(invalid());
            }
            state.signature = Some(signature.into());
        }
        state.closed = call.get("willContinue").and_then(Value::as_bool) != Some(true);
        if state.closed && state.argument_state.has_open_fields() {
            return Err(invalid());
        }
        Ok((index, is_new))
    }

    pub(in crate::protocol::adapter) fn get(&self, index: usize) -> &GeminiFunctionCall {
        &self.calls[index]
    }

    pub(in crate::protocol::adapter) fn has_open_calls(&self) -> bool {
        self.calls.iter().any(|call| !call.closed)
    }
}

impl GeminiFunctionCall {
    pub(in crate::protocol::adapter) fn native_part(&self) -> Value {
        let mut part = json!({"functionCall":{"name": self.name, "args": self.arguments}});
        if let Some(id) = &self.provider_id {
            part["functionCall"]["id"] = id.clone().into();
        }
        if let Some(signature) = &self.signature {
            part["thoughtSignature"] = signature.clone().into();
        }
        part
    }
}

/// Restore optional provider IDs on the wire. Synthetic IDs stay local.
pub(in crate::protocol::adapter) fn restore_provider_call_ids(
    contents: &mut Value,
    call_ids: &std::collections::BTreeMap<String, Option<String>>,
) -> AdapterResult<()> {
    let messages = contents
        .as_array_mut()
        .ok_or_else(AdapterError::continuation_mismatch)?;
    for message in messages {
        let parts = message
            .get_mut("parts")
            .and_then(Value::as_array_mut)
            .ok_or_else(AdapterError::continuation_mismatch)?;
        for part in parts {
            for field in ["functionCall", "functionResponse"] {
                let Some(call) = part.get_mut(field).and_then(Value::as_object_mut) else {
                    continue;
                };
                let provider_id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| call_ids.get(id));
                match provider_id {
                    Some(Some(id)) => {
                        call.insert("id".into(), id.clone().into());
                    }
                    Some(None) => {
                        call.remove("id");
                    }
                    None => {}
                }
            }
        }
    }
    Ok(())
}

mod json_path;
mod request;
mod response;

pub(in crate::protocol::adapter) use json_path::function_call_args;
#[cfg(test)]
pub use request::prepare_responses_to_gemini;
pub(crate) use request::prepare_responses_to_gemini_with_capabilities;
#[cfg(test)]
pub(crate) use request::prepare_responses_to_gemini_with_reasoning;
pub(crate) use response::gemini_incomplete;
pub use response::translate_gemini_response;
pub(in crate::protocol::adapter) use response::translate_gemini_response_with_call_ids;
pub(crate) use response::{candidate_incomplete_reason, prompt_blocked};
#[cfg(test)]
mod tests;

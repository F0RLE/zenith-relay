//! Shared, loss-checked conversion contracts. Native traffic never enters this module.
mod citations;
mod continuation;
mod decode;
mod encode;
mod model;
mod options;
mod response;
mod schema;
mod stream;
#[cfg(test)]
mod tests;
mod tool_arguments;
mod tool_search;

pub(super) use super::contracts::validate_bridge_fields as checked;
use super::contracts::{
    AdapterContinuationState, AdapterError, AdapterRequestContext, AdapterResult, ClientToolTarget,
    ResponsesToolKind, ToolChoice, TranslatedResponse,
};
use crate::{MessagesReasoningMode, WireApi};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::protocol::adapter) use model::{Block, Message};
use model::{Finish, Function, OutputFormat, Reasoning, Request, Response, Role, Usage};
pub use stream::TranslationStream;

pub(crate) fn continuation_key(
    client: WireApi,
    request: &Value,
    model: &str,
) -> AdapterResult<Option<String>> {
    continuation::request_key(client, request, model)
}

pub(in crate::protocol::adapter) fn responses_history(
    input: &Value,
) -> AdapterResult<Vec<Message>> {
    decode::responses_history(input)
}

pub(in crate::protocol::adapter) fn validate_responses_history(input: &Value) -> AdapterResult<()> {
    let mut messages = responses_history(input)?;
    decode::resolve_tool_history(&mut messages)
}

#[derive(Clone, Debug)]
pub struct TranslationRequest {
    pub(super) upstream_body: Value,
    metadata: BTreeMap<String, String>,
    client: WireApi,
    include_stream_usage: bool,
    upstream: WireApi,
    model: String,
    response_id: String,
    reasoning_mode: MessagesReasoningMode,
    history: Vec<Message>,
    client_tools: BTreeMap<String, ClientToolTarget>,
    allowed_tool_names: BTreeSet<String>,
    requires_tool_call: bool,
    parallel_tools: bool,
    response_tools: Option<Vec<Value>>,
    response_call_targets: BTreeMap<String, ClientToolTarget>,
    native_turns: BTreeMap<usize, Vec<Value>>,
}

impl TranslationRequest {
    pub(super) fn prepare(
        context: AdapterRequestContext<'_>,
        upstream: WireApi,
        capabilities: Option<&crate::model_metadata::ModelCapabilities>,
    ) -> AdapterResult<Self> {
        if context.client_wire_api == WireApi::GeminiInteractions
            || upstream == WireApi::GeminiInteractions
        {
            return Err(AdapterError::parameter_unsupported_for(
                "gemini_interactions.native_only",
            ));
        }
        let mut effective_request = std::borrow::Cow::Borrowed(context.request);
        let response_tools = if context.client_wire_api == WireApi::Responses {
            let supplied = super::contracts::request_tool_catalog(
                context
                    .request
                    .as_object()
                    .ok_or_else(AdapterError::invalid_request)?,
            )?;
            if supplied.is_none()
                && context
                    .request
                    .get("previous_response_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
            {
                if let Some(tools) = context
                    .previous
                    .as_ref()
                    .and_then(|previous| previous.response_tools.clone())
                {
                    effective_request.to_mut()["tools"] = Value::Array(tools);
                }
            }
            super::contracts::request_tool_catalog(
                effective_request.as_object().expect("request object"),
            )?
        } else {
            None
        };
        let has_previous = context.client_wire_api == WireApi::Responses
            && context
                .request
                .get("previous_response_id")
                .and_then(Value::as_str)
                .is_some_and(|response_id_value| !response_id_value.is_empty());
        if has_previous && context.previous.is_none() {
            return Err(AdapterError::continuation_missing());
        }
        let response_call_targets = if context.client_wire_api == WireApi::Responses {
            decode::responses_tool_results(&effective_request, context.previous.as_ref())?
        } else {
            BTreeMap::new()
        };
        let mut decoded_request = decode::request(context.client_wire_api, &effective_request)?;
        for target in response_call_targets
            .values()
            .filter(|target| target.kind == ResponsesToolKind::ToolSearch)
        {
            let name = target.upstream_name();
            if decoded_request
                .client_tools
                .get(&name)
                .is_some_and(|existing| existing != target)
            {
                return Err(AdapterError::continuation_mismatch());
            }
            decoded_request.client_tools.insert(name, target.clone());
        }
        let native_turns = if let Some(previous) = context.previous {
            continuation::restore(
                &mut decoded_request,
                previous,
                context.client_wire_api,
                upstream,
                context.model,
                context.reasoning_mode,
            )?
        } else {
            // Plain tool history is portable for a confirmed non-reasoning model.
            // Otherwise these protocols may require opaque assistant state that
            // the client cannot reconstruct after a restart or cache expiry.
            if upstream != WireApi::ChatCompletions
                && !capabilities.is_some_and(|capabilities| capabilities.reasoning == Some(false))
                && decoded_request
                    .messages
                    .iter()
                    .flat_map(|message| &message.blocks)
                    .any(|block| matches!(block, Block::ToolResult { .. }))
            {
                return Err(AdapterError::continuation_missing());
            }
            BTreeMap::new()
        };
        decode::resolve_tool_history(&mut decoded_request.messages)?;
        let mut upstream_body = encode::request(
            &decoded_request,
            upstream,
            context.model,
            context.stream,
            &native_turns,
            context.reasoning_mode,
            capabilities,
        )?;
        if upstream == WireApi::Messages {
            super::messages::apply_cache_write_ttl(&mut upstream_body, context.cache_write_ttl)?;
        }
        let response_id = super::messages::bridged_response_id_scoped(
            context.response_scope,
            context.response_id_seed,
        );
        Ok(Self {
            upstream_body,
            metadata: decoded_request.metadata,
            client: context.client_wire_api,
            include_stream_usage: context.client_wire_api == WireApi::ChatCompletions
                && context
                    .request
                    .pointer("/stream_options/include_usage")
                    .and_then(Value::as_bool)
                    == Some(true),
            upstream,
            model: context.model.to_owned(),
            response_id,
            reasoning_mode: context.reasoning_mode,
            history: decoded_request.messages,
            client_tools: decoded_request.client_tools,
            requires_tool_call: matches!(
                decoded_request.tool_choice.as_ref(),
                Some(ToolChoice::Required | ToolChoice::Function(_))
            ),
            allowed_tool_names: decoded_request
                .tools
                .into_iter()
                .filter_map(|tool| match &decoded_request.tool_choice {
                    Some(ToolChoice::None) => None,
                    Some(ToolChoice::Function(name)) if name != &tool.name => None,
                    _ => Some(tool.name),
                })
                .collect(),
            parallel_tools: decoded_request.parallel_tools.unwrap_or(true),
            response_tools,
            response_call_targets,
            native_turns,
        })
    }

    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }

    pub(super) fn translate(
        mut self,
        upstream_response: &Value,
    ) -> AdapterResult<TranslatedResponse> {
        let decoded_response =
            response::decode(self.upstream, upstream_response, &self.response_id)?;
        self.complete(decoded_response)
    }

    fn complete(&mut self, mut response: Response) -> AdapterResult<TranslatedResponse> {
        self.validate_tool_calls(&response.blocks)?;
        let has_calls = response
            .blocks
            .iter()
            .any(|block| matches!(block, Block::ToolCall { .. }));
        if (response.finish == Finish::Tools && !has_calls)
            || (response.finish == Finish::Stop && has_calls)
        {
            return Err(AdapterError::upstream_response_invalid().with_parameter("finish_reason"));
        }
        if self.requires_tool_call && response.finish == Finish::Stop && !has_calls {
            return Err(AdapterError::upstream_response_invalid().with_parameter("tool_choice"));
        }
        for block in &mut response.blocks {
            self.normalize_function_arguments(block, true);
        }
        response.id = self.response_id.clone();
        let mut response_body =
            response::encode(self.client, &response, &self.model, &self.client_tools)?;
        if self.client == WireApi::Responses {
            options::write_metadata(
                response_body.as_object_mut().expect("encoded response"),
                &self.metadata,
                self.client,
                None,
            )?;
        }
        let mut continuation = AdapterContinuationState::new(&self.model, self.reasoning_mode);
        continuation.client_wire_api = Some(self.client);
        continuation.reusable = matches!(response.finish, Finish::Stop | Finish::Tools);
        let mut history = std::mem::take(&mut self.history);
        continuation.native_turns = std::mem::take(&mut self.native_turns);
        if !response.native_output.is_empty() {
            continuation
                .native_turns
                .insert(history.len(), response.native_output);
        }
        continuation.upstream_protocol = Some(self.upstream);
        history.push(Message {
            role: Role::Assistant,
            blocks: response.blocks,
        });
        continuation.portable_history = Some(history);
        continuation.response_tools = self.response_tools.take();
        continuation.response_call_targets = std::mem::take(&mut self.response_call_targets);
        for block in continuation
            .portable_history
            .as_deref()
            .unwrap_or_default()
            .iter()
            .flat_map(|message| &message.blocks)
        {
            if let Block::ToolCall { id, name, .. } = block {
                if let Some(target) = self.client_tools.get(name) {
                    continuation
                        .response_call_targets
                        .entry(id.clone())
                        .or_insert_with(|| target.clone());
                }
            }
        }
        if continuation.reusable {
            continuation.history_key = Some(continuation::history_key(
                self.client,
                &self.model,
                continuation
                    .portable_history
                    .as_deref()
                    .expect("portable history"),
            )?);
        }
        Ok(TranslatedResponse {
            response_body,
            response_id: self.response_id.clone(),
            continuation,
        })
    }

    fn normalize_function_arguments(&self, block: &mut Block, complete: bool) {
        if let Block::ToolCall {
            name, arguments, ..
        } = block
        {
            match self.normalized_function_arguments(name, arguments, complete) {
                std::borrow::Cow::Borrowed(prefix) => {
                    let retained = prefix.len();
                    arguments.truncate(retained);
                }
                std::borrow::Cow::Owned(normalized) => *arguments = normalized,
            }
        }
    }

    fn normalized_function_arguments<'a>(
        &self,
        name: &str,
        arguments: &'a str,
        complete: bool,
    ) -> std::borrow::Cow<'a, str> {
        if self.client == WireApi::Responses
            && !self
                .client_tools
                .get(name)
                .is_some_and(|target| target.kind == ResponsesToolKind::Custom)
        {
            tool_arguments::normalize(arguments, complete)
        } else {
            std::borrow::Cow::Borrowed(arguments)
        }
    }

    fn validate_tool_call(&self, id: &str, name: &str) -> AdapterResult<()> {
        if id.trim().is_empty()
            || self.history.iter().flat_map(|message| &message.blocks).any(
                |block| matches!(block, Block::ToolCall { id: previous_id, .. } if previous_id == id),
            )
        {
            return Err(
                AdapterError::upstream_response_invalid().with_parameter("tool_calls.id")
            );
        }
        if !self.allowed_tool_names.contains(name) {
            return Err(AdapterError::upstream_response_invalid()
                .with_parameter("tool_calls.function.name"));
        }
        Ok(())
    }

    fn validate_tool_calls(&self, blocks: &[Block]) -> AdapterResult<()> {
        let mut calls = 0;
        for block in blocks {
            if let Block::ToolCall { id, name, .. } = block {
                self.validate_tool_call(id, name)?;
                calls += 1;
            }
        }
        if !self.parallel_tools && calls > 1 {
            return Err(AdapterError::upstream_response_invalid());
        }
        Ok(())
    }
}

/// Compatible Chat providers use either spelling for the same plaintext.
/// Conflicting nonempty values cannot be merged without changing their meaning.
fn chat_reasoning_text(message: &Value, invalid: AdapterError) -> AdapterResult<Option<&str>> {
    let text = |field| {
        message
            .get(field)
            .filter(|value| !value.is_null())
            .map(|value| value.as_str().ok_or(invalid))
            .transpose()
            .map(|value| value.filter(|text| !text.is_empty()))
    };
    match (text("reasoning_content")?, text("reasoning")?) {
        (Some(content), Some(reasoning)) if content != reasoning => Err(invalid),
        (Some(content), _) => Ok(Some(content)),
        (_, reasoning) => Ok(reasoning),
    }
}

fn required_text<'a>(json_object: &'a Value, key: &str) -> AdapterResult<&'a str> {
    json_object
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(AdapterError::invalid_request)
}

/// Other protocols expose synchronous, directly executed calls. Explicitly
/// equivalent Responses metadata is harmless; async/program execution is not.
fn validate_direct_tool_execution(
    call: &Value,
    async_error: AdapterError,
    caller_error: AdapterError,
) -> AdapterResult<()> {
    if call
        .get("async")
        .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false))
    {
        return Err(async_error);
    }
    if let Some(caller) = call.get("caller").filter(|value| !value.is_null()) {
        checked(caller, &["type"]).map_err(|_| caller_error)?;
        if caller.get("type").and_then(Value::as_str) != Some("direct") {
            return Err(caller_error);
        }
    }
    Ok(())
}

fn optional_u64(json_object: &Value, key: &str) -> AdapterResult<Option<u64>> {
    json_object
        .get(key)
        .filter(|field_value| !field_value.is_null())
        .map(|field_value| {
            field_value
                .as_u64()
                .ok_or_else(AdapterError::invalid_request)
        })
        .transpose()
}

fn optional_f64(json_object: &Value, key: &str) -> AdapterResult<Option<f64>> {
    json_object
        .get(key)
        .filter(|field_value| !field_value.is_null())
        .map(|field_value| {
            field_value
                .as_f64()
                .ok_or_else(AdapterError::invalid_request)
        })
        .transpose()
}

fn optional_bool(json_object: &Value, key: &str) -> AdapterResult<Option<bool>> {
    json_object
        .get(key)
        .filter(|field_value| !field_value.is_null())
        .map(|field_value| {
            field_value
                .as_bool()
                .ok_or_else(AdapterError::invalid_request)
        })
        .transpose()
}

//! Shared, loss-checked conversion contracts. Native traffic never enters this module.
mod citations;
mod continuation;
mod decode;
mod encode;
mod options;
mod response;
mod schema;
mod stream;
#[cfg(test)]
mod tests;
mod tool_arguments;

pub(super) use super::contracts::validate_bridge_fields as checked;
use super::contracts::{
    AdapterError, AdapterRequestContext, AdapterResult, ClientToolTarget, MessagesBridgeResponse,
    MessagesBridgeState, ResponsesToolKind, ToolChoice,
};
use crate::{MessagesReasoningMode, WireApi};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) enum Block {
    Text(String),
    Image {
        url: String,
        detail: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        id: String,
        name: String,
        content: Vec<Block>,
        is_error: bool,
        /// Custom-tool output is literal text, even when it looks like JSON.
        raw_text: bool,
    },
    Reasoning(String),
    Refusal(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(super) enum Role {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Message {
    role: Role,
    blocks: Vec<Block>,
}

#[derive(Clone, Debug)]
struct Function {
    name: String,
    description: Option<String>,
    parameters: Value,
    strict: Option<bool>,
}

#[derive(Clone, Debug)]
enum OutputFormat {
    JsonObject,
    JsonSchema {
        name: String,
        description: Option<String>,
        schema: Value,
        strict: Option<bool>,
    },
}

#[derive(Clone, Debug)]
enum Reasoning {
    Effort(String),
    Budget(u64),
    Adaptive,
    DynamicBudget,
}

#[derive(Clone, Debug, Default)]
struct Request {
    origin: Option<WireApi>,
    metadata: BTreeMap<String, String>,
    instructions: Option<Message>,
    messages: Vec<Message>,
    tools: Vec<Function>,
    /// Responses client identity (kind, local name, namespace) for each
    /// upstream tool name. Only a Responses client fills it.
    client_tools: BTreeMap<String, ClientToolTarget>,
    tool_choice: Option<ToolChoice>,
    parallel_tools: Option<bool>,
    max_tokens: Option<u64>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    top_k: Option<u64>,
    presence_penalty: Option<f64>,
    frequency_penalty: Option<f64>,
    seed: Option<i64>,
    stop: Vec<String>,
    output_format: Option<OutputFormat>,
    reasoning: Option<Reasoning>,
}

#[derive(Clone, Debug, Default)]
struct Usage {
    input: Option<u64>,
    output: Option<u64>,
    total: Option<u64>,
    cached: Option<u64>,
    reasoning: Option<u64>,
    cache_write: Option<u64>,
    cache_write_5m: Option<u64>,
    cache_write_1h: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Finish {
    Stop,
    Tools,
    Length,
    Filter,
    Refusal,
}

#[derive(Clone, Debug)]
struct Response {
    id: String,
    blocks: Vec<Block>,
    citations: BTreeMap<usize, Vec<citations::UrlCitation>>,
    usage: Usage,
    finish: Finish,
    /// Exact provider-owned assistant items, separate from portable content.
    /// They are only replayed to the source protocol that produced them.
    native_output: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct TranslationRequest {
    pub(super) upstream_body: Value,
    metadata: BTreeMap<String, String>,
    client: WireApi,
    upstream: WireApi,
    model: String,
    response_id: String,
    reasoning_mode: MessagesReasoningMode,
    history: Vec<Message>,
    client_tools: BTreeMap<String, ClientToolTarget>,
    allowed_tool_names: BTreeSet<String>,
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
            upstream,
            model: context.model.to_owned(),
            response_id,
            reasoning_mode: context.reasoning_mode,
            history: decoded_request.messages,
            client_tools: decoded_request.client_tools,
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
        self,
        upstream_response: &Value,
    ) -> AdapterResult<MessagesBridgeResponse> {
        let decoded_response =
            response::decode(self.upstream, upstream_response, &self.response_id)?;
        self.complete(decoded_response)
    }

    fn complete(self, mut response: Response) -> AdapterResult<MessagesBridgeResponse> {
        self.validate_tool_calls(&response.blocks)?;
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
            )?;
        }
        let mut continuation = MessagesBridgeState::new(&self.model, self.reasoning_mode);
        continuation.client_wire_api = Some(self.client);
        continuation.reusable = matches!(response.finish, Finish::Stop | Finish::Tools);
        let mut history = self.history;
        continuation.native_turns = self.native_turns;
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
        continuation.response_tools = self.response_tools;
        continuation.response_call_targets = self.response_call_targets;
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
        continuation.history_key = Some(continuation::history_key(
            self.client,
            &self.model,
            continuation
                .portable_history
                .as_deref()
                .expect("portable history"),
        )?);
        Ok(MessagesBridgeResponse {
            response_body,
            response_id: self.response_id,
            continuation,
        })
    }

    fn normalize_function_arguments(&self, block: &mut Block, complete: bool) {
        if self.client != WireApi::Responses {
            return;
        }
        if let Block::ToolCall {
            name, arguments, ..
        } = block
        {
            if !self
                .client_tools
                .get(name)
                .is_some_and(|target| target.kind == ResponsesToolKind::Custom)
            {
                *arguments = tool_arguments::normalize(arguments, complete);
            }
        }
    }

    fn validate_tool_call(&self, id: &str, name: &str) -> AdapterResult<()> {
        if id.is_empty()
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

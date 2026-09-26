//! Shared, loss-checked conversion contracts. Native traffic never enters this module.
mod decode;
mod encode;
mod response;
mod stream;
#[cfg(test)]
mod tests;

use super::contracts::{
    AdapterError, AdapterRequestContext, AdapterResult, MessagesBridgeResponse, MessagesBridgeState,
};
use crate::{MessagesReasoningMode, WireApi};
use serde::Serialize;
use serde_json::Value;

pub use stream::TranslationStream;

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
        content: String,
        is_error: bool,
    },
    Reasoning(String),
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
enum ToolChoice {
    Auto,
    None,
    Required,
    Function(String),
}

#[derive(Clone, Debug)]
enum OutputFormat {
    JsonObject,
    JsonSchema {
        name: String,
        schema: Value,
        strict: Option<bool>,
    },
}

#[derive(Clone, Debug)]
enum Reasoning {
    Effort(String),
    Budget(u64),
}

#[derive(Clone, Debug, Default)]
struct Request {
    instructions: Option<Message>,
    messages: Vec<Message>,
    tools: Vec<Function>,
    tool_choice: Option<ToolChoice>,
    parallel_tools: Option<bool>,
    max_tokens: Option<u64>,
    temperature: Option<f64>,
    top_p: Option<f64>,
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
}

#[derive(Clone, Debug)]
struct Response {
    id: String,
    blocks: Vec<Block>,
    usage: Usage,
    finish: Finish,
}

#[derive(Clone, Debug)]
pub struct TranslationRequest {
    pub(super) upstream_body: Value,
    client: WireApi,
    upstream: WireApi,
    model: String,
    response_id: String,
    reasoning_mode: MessagesReasoningMode,
    history: Vec<Message>,
}

impl TranslationRequest {
    pub(super) fn prepare(
        context: AdapterRequestContext<'_>,
        upstream: WireApi,
    ) -> AdapterResult<Self> {
        let mut request = decode::request(context.client_wire_api, context.request)?;
        if context.client_wire_api == WireApi::Responses
            && context
                .request
                .get("previous_response_id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty())
        {
            let previous = context
                .previous
                .ok_or_else(AdapterError::continuation_missing)?;
            if previous.model != context.model {
                return Err(AdapterError::continuation_mismatch());
            }
            if previous.reasoning_mode != context.reasoning_mode {
                return Err(AdapterError::continuation_mismatch());
            }
            let mut history = previous
                .portable_history
                .ok_or_else(AdapterError::continuation_mismatch)?;
            history.append(&mut request.messages);
            request.messages = history;
        }
        decode::resolve_tool_history(&mut request.messages)?;
        let mut upstream_body = encode::request(&request, upstream, context.model, context.stream)?;
        if upstream == WireApi::Messages {
            super::messages::apply_cache_write_ttl(&mut upstream_body, context.cache_write_ttl)?;
        }
        let response_id = super::messages::bridged_response_id_scoped(
            context.response_scope,
            context.response_id_seed,
        );
        Ok(Self {
            upstream_body,
            client: context.client_wire_api,
            upstream,
            model: context.model.to_owned(),
            response_id,
            reasoning_mode: context.reasoning_mode,
            history: request.messages,
        })
    }

    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }

    pub(super) fn translate(self, value: &Value) -> AdapterResult<MessagesBridgeResponse> {
        let response = response::decode(self.upstream, value, &self.response_id)?;
        self.complete(response)
    }

    fn complete(self, mut response: Response) -> AdapterResult<MessagesBridgeResponse> {
        response.id = self.response_id.clone();
        let response_body = response::encode(self.client, &response, &self.model)?;
        let mut continuation = MessagesBridgeState::new(&self.model, self.reasoning_mode);
        let mut history = self.history;
        history.push(Message {
            role: Role::Assistant,
            blocks: response.blocks,
        });
        continuation.portable_history = Some(history);
        Ok(MessagesBridgeResponse {
            response_body,
            response_id: self.response_id,
            continuation,
        })
    }
}

fn required_text<'a>(value: &'a Value, key: &str) -> AdapterResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(AdapterError::invalid_request)
}

fn checked(value: &Value, allowed: &[&str]) -> AdapterResult<()> {
    let object = value
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    if object
        .iter()
        .any(|(key, value)| !allowed.contains(&key.as_str()) && !value.is_null())
    {
        return Err(AdapterError::parameter_unsupported());
    }
    Ok(())
}

fn optional_u64(value: &Value, key: &str) -> AdapterResult<Option<u64>> {
    value
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| value.as_u64().ok_or_else(AdapterError::invalid_request))
        .transpose()
}

fn optional_f64(value: &Value, key: &str) -> AdapterResult<Option<f64>> {
    value
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| value.as_f64().ok_or_else(AdapterError::invalid_request))
        .transpose()
}

fn optional_bool(value: &Value, key: &str) -> AdapterResult<Option<bool>> {
    value
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| value.as_bool().ok_or_else(AdapterError::invalid_request))
        .transpose()
}

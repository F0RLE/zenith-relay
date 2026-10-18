//! Portable conversation semantics shared by the four protocol codecs.
//! Provider-owned blocks stay in `Response::native_output`, never in text.

use super::{citations, ClientToolTarget, ToolChoice, WireApi};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(in crate::protocol::adapter) enum Block {
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
pub(in crate::protocol::adapter) enum Role {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(in crate::protocol::adapter) struct Message {
    pub(super) role: Role,
    pub(super) blocks: Vec<Block>,
}

#[derive(Clone, Debug)]
pub(super) struct Function {
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) parameters: Value,
    pub(super) strict: Option<bool>,
}

#[derive(Clone, Debug)]
pub(super) enum OutputFormat {
    JsonObject,
    JsonSchema {
        name: String,
        description: Option<String>,
        schema: Value,
        strict: Option<bool>,
    },
}

#[derive(Clone, Debug)]
pub(super) enum Reasoning {
    Effort(String),
    Budget(u64),
    Adaptive,
    DynamicBudget,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Request {
    pub(super) origin: Option<WireApi>,
    pub(super) metadata: BTreeMap<String, String>,
    pub(super) instructions: Option<Message>,
    pub(super) messages: Vec<Message>,
    pub(super) tools: Vec<Function>,
    /// Responses tool kind, local name and namespace, keyed by upstream name.
    pub(super) client_tools: BTreeMap<String, ClientToolTarget>,
    pub(super) tool_choice: Option<ToolChoice>,
    pub(super) parallel_tools: Option<bool>,
    pub(super) max_tokens: Option<u64>,
    pub(super) temperature: Option<f64>,
    pub(super) top_p: Option<f64>,
    pub(super) top_k: Option<u64>,
    pub(super) presence_penalty: Option<f64>,
    pub(super) frequency_penalty: Option<f64>,
    pub(super) seed: Option<i64>,
    pub(super) stop: Vec<String>,
    pub(super) output_format: Option<OutputFormat>,
    pub(super) reasoning: Option<Reasoning>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Usage {
    pub(super) input: Option<u64>,
    pub(super) output: Option<u64>,
    pub(super) total: Option<u64>,
    pub(super) cached: Option<u64>,
    pub(super) reasoning: Option<u64>,
    pub(super) cache_write: Option<u64>,
    pub(super) cache_write_5m: Option<u64>,
    pub(super) cache_write_1h: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Finish {
    Stop,
    Tools,
    Length,
    Filter,
    Refusal,
}

#[derive(Clone, Debug)]
pub(super) struct Response {
    pub(super) id: String,
    pub(super) blocks: Vec<Block>,
    pub(super) citations: BTreeMap<usize, Vec<citations::UrlCitation>>,
    pub(super) usage: Usage,
    pub(super) finish: Finish,
    /// Exact provider-owned assistant items; replay only to their source API.
    pub(super) native_output: Vec<Value>,
}

//! The Excel/Basis Points account transport.
//!
//! Basis Points exposes a Responses-shaped endpoint, but its client-tool
//! contract is different: client tools are invoked through one native
//! `run_officejs` function. Keep that protocol detail inside this executor so
//! the rest of the gateway can continue to operate on ordinary Responses
//! requests and responses.
//!
//! `catalog` reads the client tool list, `codec` owns the small JSON helpers,
//! `history` rewrites prior calls, `prepare` builds the upstream request, and
//! `response` turns the native tool relay back into the client protocol.

mod catalog;
mod codec;
mod history;
mod prepare;
mod response;

pub(in crate::gateway::execution) use prepare::{
    add_tool_relay_retry_hint, prepare_request, prepare_upstream, take_tool_relay_retry,
};
pub(in crate::gateway::execution) use response::{synthetic_stream, translate_response};

pub(super) const TRANSPORT_TOOL: &str = "run_officejs";
pub(super) const TRANSPORT_TOOL_ALIAS: &str = "functions.run_officejs";
pub(super) const TRANSPORT_RETRY_HINT: &str = "The previous run_officejs relay was malformed. Retry once using exactly one client tool name in outer references and only its payload in code: a JSON arguments object for function tools, or unchanged raw input for custom tools. Do not wrap the payload in a tool/args object. Serialize the outer arguments once, including quotes and backslashes.";

#[cfg(test)]
mod tests;

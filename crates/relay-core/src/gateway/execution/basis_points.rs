//! The Excel/Basis Points account transport.
//!
//! Basis Points exposes a Responses-shaped endpoint, but its client-tool
//! contract is different: client tools are invoked through one native
//! `run_officejs` function. Keep that protocol detail inside this executor so
//! the rest of the gateway can continue to operate on ordinary Responses
//! requests and responses.
//!
//! `catalog` reads the client tool list, `codec` owns the small JSON helpers,
//! `history` rewrites prior calls, `prepare` builds the upstream request,
//! `attachments` uploads user images, and `response` turns the native tool
//! relay back into the client protocol.

mod attachments;
mod catalog;
mod codec;
mod history;
mod prepare;
mod response;
mod schema;

pub(in crate::gateway::execution) use attachments::{attach_input_images, AttachmentFailure};
pub(in crate::gateway::execution) use prepare::prepare_request;
pub(in crate::gateway::execution) use response::{synthetic_stream, translate_response};

pub(super) const TRANSPORT_TOOL: &str = "run_officejs";
pub(super) const TRANSPORT_TOOL_ALIAS: &str = "functions.run_officejs";

pub(super) fn is_transport_tool(tool_name: &str) -> bool {
    tool_name == TRANSPORT_TOOL || tool_name == TRANSPORT_TOOL_ALIAS
}
/// Function arguments and the outer `run_officejs` arguments are two JSON
/// layers. The adapter's v0.2.10 instructions say this explicitly so a function
/// `apply_patch` is not taught as raw custom text.
pub(super) const FUNCTION_RELAY_ENCODING: &str = concat!(
    "For function tools, first JSON-serialize the complete arguments object, then use that JSON text as the outer code string and serialize the outer arguments object. ",
    "These are two distinct JSON layers: preserve escaped quotes, backslashes, newlines and tabs in string arguments at both layers. ",
    "The decoded code must itself parse as one JSON object. ",
    "A function with a single patch, code, cmd or input field still requires the object wrapper named by its schema; raw patch or script text is only valid for a custom tool.",
);

#[cfg(test)]
mod tests;

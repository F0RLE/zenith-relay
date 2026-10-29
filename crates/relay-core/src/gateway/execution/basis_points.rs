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

mod agent_messages;
mod attachments;
mod batch;
mod catalog;
mod codec;
pub(in crate::gateway) mod discovery;
mod history;
mod prepare;
mod presentation;
pub(in crate::gateway) mod response;
mod schema;

/// Diagnostics and idle cleanup never initialize a cache or wait on a request.
/// Returned values are dropped after releasing the cache lock.
fn try_with_cache<T, R: Default>(
    cache: &std::sync::OnceLock<std::sync::Mutex<T>>,
    operation: impl FnOnce(&mut T) -> R,
) -> Option<R> {
    let Some(cache) = cache.get() else {
        return Some(R::default());
    };
    let mut cache = cache.try_lock().ok()?;
    Some(operation(&mut cache))
}

fn describe_cache<T>(
    cache: &std::sync::OnceLock<std::sync::Mutex<T>>,
    key: &'static str,
    limit_bytes: usize,
    basis: &'static str,
    measure: impl FnOnce(&mut T) -> (usize, usize),
) -> crate::gateway::CacheMemoryDiagnostics {
    let usage = try_with_cache(cache, measure);
    crate::gateway::CacheMemoryDiagnostics {
        key,
        entries: usage.map(|(entries, _)| entries),
        bytes: usage.map(|(_, bytes)| bytes),
        limit_bytes: Some(limit_bytes),
        basis,
    }
}

pub(in crate::gateway) fn cache_memory_diagnostics() -> Vec<crate::gateway::CacheMemoryDiagnostics>
{
    vec![
        catalog::cache_memory_diagnostics(),
        schema::cache_memory_diagnostics(),
    ]
}

pub(in crate::gateway) fn release_expired_cache_entries() {
    catalog::release_expired_cache_entries();
    schema::release_expired_cache_entries();
}
pub(in crate::gateway::execution) mod structured_output;

pub(in crate::gateway::execution) use attachments::{AttachmentFailure, attach_input_images};
pub(in crate::gateway) use catalog::{PreparedClientRequest, RequestCatalog};
#[cfg(test)]
pub(in crate::gateway::execution) use prepare::prepare_request_with_timing;
pub(in crate::gateway::execution) use prepare::{
    output_format_error, prepare_request, prepare_request_with_catalog,
};
pub(in crate::gateway) use response::synthetic_stream;
#[cfg(test)]
pub(in crate::gateway) use response::translate_response;

pub(super) const TRANSPORT_TOOL: &str = "run_officejs";
pub(super) const TRANSPORT_TOOL_ALIAS: &str = "functions.run_officejs";
pub(super) const FUNCTION_STRING_PREFIX: &str = "relay-function-string-v1 ";

pub(super) fn is_transport_tool(tool_name: &str) -> bool {
    tool_name == TRANSPORT_TOOL || tool_name == TRANSPORT_TOOL_ALIAS
}
/// Frame complex string arguments explicitly instead of nesting their JSON escapes.
pub(super) const FUNCTION_RELAY_ENCODING: &str = concat!(
    "For function tools with multiline strings, embedded quotes or backslashes, use a framed code payload instead of nested JSON. ",
    "Its first line is relay-function-string-v1 followed by a space and a JSON header {\"field\":\"FIELD\",\"args\":{}}; after one actual newline, put the exact raw string value for FIELD. ",
    "FIELD must name the string argument from the tool schema. Put all other arguments in args and do not repeat FIELD there. ",
    "Keep the header compact on one line and JSON-escape every string in args; only the body is raw. Prefer framing the command/script, not workdir or a path: commands and their quotes then stay out of the JSON header. Other string arguments still need JSON escaping. ",
    "Do not put JSON quotes around the raw body or escape it as inner JSON. Relay reconstructs the complete arguments object and validates the original schema. Serialize the outer native arguments object once. ",
    "For ordinary JSON-form function tools, first JSON-serialize the complete arguments object, then use that JSON text as the outer code string and serialize the outer arguments object. ",
    "These are two distinct JSON layers: preserve escaped quotes, backslashes, newlines and tabs in string arguments at both layers. ",
    "In JSON form the decoded code must itself parse as one JSON object. ",
    "A function with a single patch, code, cmd or input field still needs that schema field, either in the explicit frame header or in the JSON object. Bare unframed patch or script text is only valid for a custom tool.",
);

#[cfg(test)]
mod tests;

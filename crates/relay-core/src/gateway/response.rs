mod completion;
mod http;
mod usage;

pub(in crate::gateway) use completion::{
    BufferedResponseTiming, collect_basis_points_response, collect_upstream_response_with_prewarm,
    stop_basis_points_event,
};
pub(super) use completion::{
    collect_upstream_response, completed_upstream_response, validate_upstream_response,
};
pub(super) use http::{
    CompletionCallback, attach_error_diagnostics, attach_stream_diagnostics, proxy_error_response,
    proxy_json_response, proxy_response, proxy_sse_response, route_error_origin,
    upstream_body_error_response,
};
pub(super) use usage::{
    UsageAttempt, apply_usage, emit_callback, emit_repair, emit_usage, find_usage,
    has_generated_tokens, populate_tokens, response_id, response_id_from_bytes,
    response_service_tier, usage_event,
};

#[cfg(test)]
use usage::cache_write_ttl_from_usage;

#[cfg(test)]
mod tests;

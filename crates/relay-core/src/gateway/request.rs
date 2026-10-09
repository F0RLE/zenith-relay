use crate::error_codes;
mod account;
mod codex_models;
mod headers;
mod normalization;

mod background;
mod dispatch;
mod responses_items;
mod routing;
mod tool_policy;

pub(super) use background::codex_background_request_kind;
pub(super) use dispatch::{chat_completions, gemini, messages, responses};
pub(super) use responses_items::{
    contains_tool_call_output, remove_unpaired_responses_tool_call,
    repair_legacy_responses_call_ids, response_tool_call_ids, responses_item_has_ciphertext,
    tool_call_output_ids, unpaired_tool_output_ids,
};
pub(super) use routing::{
    candidate_protocols, chat_request_is_text_or_image_only, request_id, requested_reasoning_effort,
};
#[cfg(test)]
pub(super) use tool_policy::tool_use_diagnostics;
pub(in crate::gateway) use tool_policy::RequestToolPolicy;

#[cfg(test)]
mod tests;

#[cfg(test)]
use super::now_ms;
pub(super) use account::{account_endpoint_url, alpha_search, responses_compact, AccountEndpoint};
#[cfg(test)]
use codex_models::build_codex_models_response;
pub(super) use codex_models::models;
pub(super) use headers::{
    apply_codex_routing_hint, client_context_fingerprint, codex_client_version,
    forwarded_bridge_gemini_headers, forwarded_bridge_messages_headers, forwarded_codex_headers,
    forwarded_messages_headers, forwarded_responses_headers, is_managed_codex_client,
};
pub(in crate::gateway) use normalization::coerce_responses_input_array;
#[cfg(test)]
pub(super) use normalization::{apply_default_service_tier_if_missing, request_service_tier};
pub(super) use normalization::{
    normalize_account_request, normalize_basis_points_request, normalize_compact_account_request,
    normalize_responses_lite_request, responses_lite_parallel_tool_calls_valid, ServiceTierPolicy,
};

use super::execution::execute_client_request;
#[cfg(test)]
use crate::codex_catalog_entry_is_compatible;
use crate::{GatewayRuntime, ToolChoiceMode, ToolUseDiagnostics, WireApi};
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, Response};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

// Legacy replay repair is deliberately fail-closed for unusually large or
// adversarial histories. These bounds keep matching and temporary state
// predictable.
const MAX_LEGACY_RESPONSES_REPAIR_ITEMS: usize = 4_096;
const MAX_LEGACY_RESPONSES_PENDING_CALLS: usize = 256;
const MAX_LEGACY_RESPONSES_NAME_CHARS: usize = 256;

pub(super) const CODEX_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";

pub(super) const CODEX_ACTIVITY_SUMMARY: &str = "activity_summary";
pub(super) const CODEX_TASK_TITLE: &str = "task_title";

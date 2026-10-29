use crate::error_codes;
mod account;
mod codex_models;
mod headers;
mod identity;
mod normalization;

mod background;
mod dispatch;
mod responses_items;
mod routing;
mod tool_calls;
mod tool_policy;

pub(in crate::gateway) use tool_calls::ToolCallCapture;

pub(super) use background::codex_background_request_kind;
pub(super) use dispatch::{chat_completions, gemini, interactions, messages, responses};
pub(super) use responses_items::{
    contains_tool_call_output, remove_unpaired_responses_tool_call,
    repair_legacy_responses_call_ids, response_tool_call_ids, responses_item_has_ciphertext,
    tool_call_output_ids, unpaired_tool_output_ids,
};
pub(super) use routing::{
    candidate_protocols, chat_request_has_supported_content, request_id, requested_reasoning_effort,
};
pub(in crate::gateway) use tool_policy::RequestToolPolicy;
#[cfg(test)]
pub(super) use tool_policy::tool_use_diagnostics;

#[cfg(test)]
mod tests;

#[cfg(test)]
use super::now_ms;
pub(super) use account::{AccountEndpoint, account_endpoint_url, alpha_search, responses_compact};
#[cfg(test)]
use codex_models::build_codex_models_response;
pub(super) use codex_models::models;
pub(super) use headers::{
    apply_codex_routing_hint, codex_client_version, forwarded_bridge_gemini_headers,
    forwarded_bridge_messages_headers, forwarded_codex_headers, forwarded_messages_headers,
    forwarded_responses_headers, is_managed_codex_client,
};
pub(in crate::gateway) use identity::history_affinity_fingerprint;
pub(super) use identity::{client_context_fingerprint, client_request_identity};
mod codex_reasoning;
pub(in crate::gateway) use codex_reasoning::normalize_codex_reasoning_placeholder;
pub(in crate::gateway) use normalization::coerce_responses_input_array;
pub(super) use normalization::{
    ServiceTierPolicy, apply_account_prompt_cache_key, normalize_account_request,
    normalize_compact_account_request, responses_lite_parallel_tool_calls_valid,
};
#[cfg(test)]
pub(super) use normalization::{apply_default_service_tier_if_missing, request_service_tier};

use super::execution::execute_client_request;
#[cfg(test)]
use crate::codex_catalog_entry_is_compatible;
use crate::{GatewayRuntime, ToolChoiceMode, ToolUseDiagnostics, WireApi};
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, Response};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

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
pub(super) const CODEX_MEMORY_EXTRACTION: &str = "memory_extraction";
pub(super) const CODEX_MEMORY_CONSOLIDATION: &str = "memory_consolidation";

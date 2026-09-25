use crate::error_codes;
mod account;
mod codex_models;
mod headers;
mod normalization;

#[cfg(test)]
use super::now_ms;
pub(super) use account::{account_endpoint_url, alpha_search, responses_compact, AccountEndpoint};
#[cfg(test)]
use codex_models::build_codex_models_response;
pub(super) use codex_models::models;
pub(super) use headers::{
    apply_codex_routing_hint, client_context_fingerprint, codex_client_version,
    forwarded_bridge_gemini_headers, forwarded_bridge_messages_headers, forwarded_codex_headers,
    forwarded_messages_headers, is_managed_codex_client,
};
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
use axum::http::{HeaderMap, Request, Response, StatusCode};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) const MAX_CLIENT_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

// Legacy replay repair is deliberately fail-closed for unusually large or
// adversarial histories. The normal request-body limit still applies, while
// these bounds keep matching and temporary state predictable.
const MAX_LEGACY_RESPONSES_REPAIR_ITEMS: usize = 4_096;
const MAX_LEGACY_RESPONSES_PENDING_CALLS: usize = 256;
const MAX_LEGACY_RESPONSES_NAME_CHARS: usize = 256;

pub(super) const MAX_CLIENT_REQUEST_BODY_ERROR: &str = "request body exceeds 64 MiB";

const MAX_ALPHA_SEARCH_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

pub(super) const CODEX_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";

pub(super) const CODEX_ACTIVITY_SUMMARY: &str = "activity_summary";
pub(super) const CODEX_TASK_TITLE: &str = "task_title";

/// Classifies only explicit Codex background operations. A model name,
/// reasoning effort, or ordinary Codex originator is deliberately insufficient
/// evidence because users can select those values manually.
pub(super) fn codex_background_request_kind(
    headers: &HeaderMap,
    request: &Value,
) -> Option<&'static str> {
    let originator = headers
        .get("originator")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let has_codex_identity = originator.contains("codex")
        || headers.contains_key("x-codex-turn-metadata")
        || headers.contains_key("x-openai-subagent");
    if !has_codex_identity {
        return None;
    }
    if let Some(metadata) = headers
        .get("x-codex-turn-metadata")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
    {
        if metadata_has_kind(&metadata, CODEX_ACTIVITY_SUMMARY) {
            return Some(CODEX_ACTIVITY_SUMMARY);
        }
        if metadata_has_kind(&metadata, CODEX_TASK_TITLE) {
            return Some(CODEX_TASK_TITLE);
        }
    }
    if let Some(kind) = headers
        .get("x-openai-subagent")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| metadata_kind_text(value))
    {
        return Some(kind);
    }
    // Prompt text is only a fallback for clients that send the explicit
    // background marker but omit its structured value. A normal Codex request
    // can contain the same words and must not be classified from originator
    // alone.
    if headers.contains_key("x-codex-turn-metadata") || headers.contains_key("x-openai-subagent") {
        let mut strings = Vec::new();
        collect_request_strings(request.get("input"), &mut strings);
        collect_request_strings(request.get("instructions"), &mut strings);
        for text in strings {
            let normalized = text.trim().to_ascii_lowercase();
            if normalized.starts_with("summarize the activity")
                || normalized.starts_with("summarise the activity")
            {
                return Some(CODEX_ACTIVITY_SUMMARY);
            }
            if normalized.starts_with("generate a concise title for this task")
                || normalized.starts_with("generate a title for this task")
            {
                return Some(CODEX_TASK_TITLE);
            }
        }
    }
    None
}

fn metadata_has_kind(value: &Value, expected: &str) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let relevant = matches!(
                key.to_ascii_lowercase().as_str(),
                "request_type"
                    | "requesttype"
                    | "task_type"
                    | "tasktype"
                    | "purpose"
                    | "operation"
                    | "kind"
            );
            (relevant && metadata_kind(value) == Some(expected))
                || metadata_has_kind(value, expected)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| metadata_has_kind(value, expected)),
        _ => false,
    }
}

fn metadata_kind(value: &Value) -> Option<&'static str> {
    let Value::String(text) = value else {
        return None;
    };
    let normalized = text.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "activity_summary" | "activity-summary" | "activity summary" | "summarize_activity" => {
            Some(CODEX_ACTIVITY_SUMMARY)
        }
        "task_title" | "task-title" | "task title" | "generate_title" | "title_generation" => {
            Some(CODEX_TASK_TITLE)
        }
        _ => None,
    }
}

fn metadata_kind_text(text: &str) -> Option<&'static str> {
    metadata_kind(&Value::String(text.to_string()))
}

fn collect_request_strings(value: Option<&Value>, output: &mut Vec<String>) {
    match value {
        Some(Value::String(value)) => output.push(value.clone()),
        Some(Value::Array(values)) => values
            .iter()
            .for_each(|value| collect_request_strings(Some(value), output)),
        Some(Value::Object(object)) => object
            .values()
            .for_each(|value| collect_request_strings(Some(value), output)),
        _ => {}
    }
}

pub(super) fn requested_reasoning_effort(request: &Value, wire_api: WireApi) -> Option<String> {
    let effort = match wire_api {
        WireApi::Responses => request.pointer("/reasoning/effort"),
        WireApi::ChatCompletions => request.get("reasoning_effort"),
        WireApi::Messages => request.pointer("/output_config/effort"),
        WireApi::Gemini => request.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
    };
    effort
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort| !effort.is_empty() && !effort.eq_ignore_ascii_case("none"))
        .map(str::to_ascii_lowercase)
}

pub(super) async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::Responses).await
}

pub(super) async fn chat_completions(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::ChatCompletions).await
}

pub(super) async fn messages(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    super::messages::native_messages_error_response(
        execute_client_request(runtime, request, WireApi::Messages).await,
    )
    .await
}

pub(super) async fn gemini(
    State(runtime): State<Arc<GatewayRuntime>>,
    axum::extract::Path(model_action): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let Some((model, stream)) = parse_gemini_model_action(&model_action) else {
        return super::errors::api_error(
            axum::http::StatusCode::NOT_FOUND,
            "Gemini endpoint must end with :generateContent or :streamGenerateContent",
            error_codes::INVALID_REQUEST,
        );
    };
    super::execution::execute_gemini_client_request(runtime, request, model, stream).await
}

fn parse_gemini_model_action(value: &str) -> Option<(String, bool)> {
    let (model, stream) = if let Some(model) = value.strip_suffix(":streamGenerateContent") {
        (model, true)
    } else {
        let model = value.strip_suffix(":generateContent")?;
        (model, false)
    };
    let model = model.strip_prefix("models/").unwrap_or(model).trim();
    crate::is_valid_model_id(model).then(|| (model.to_string(), stream))
}

pub(super) fn contains_tool_call_output(value: &Value) -> bool {
    let mut found = false;
    visit_response_items(value, &mut |object| {
        found |= object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "tool_search_output" || kind.ends_with("_call_output"));
    });
    found
}

/// Returns the stable ids carried by Responses tool outputs. These ids are
/// stateful when the matching call is not included in the same request: the
/// provider that emitted the call is then the only safe owner.
/// IDs are length-bounded, but ownership checks must include the whole request.
/// Do not retain the tool payload itself.
pub(super) fn tool_call_output_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    visit_response_items(value, &mut |object| {
        let kind = object.get("type").and_then(Value::as_str);
        if kind.is_some_and(|kind| kind == "tool_search_output" || kind.ends_with("_call_output")) {
            if let Some(call_id) = bounded_tool_call_id(object.get("call_id")) {
                if seen.insert(call_id.clone()) {
                    ids.push(call_id);
                }
            }
        }
    });
    ids
}

/// Returns tool-call ids from a successful Responses response. Binding these
/// ids lets the next request remain on the candidate that created the call,
/// even when a client changes the selected model between turns.
pub(super) fn response_tool_call_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    visit_response_items(value, &mut |object| {
        let kind = object.get("type").and_then(Value::as_str);
        if kind.is_some_and(|kind| kind.ends_with("_call")) {
            for field in ["call_id", "id"] {
                if let Some(call_id) = bounded_tool_call_id(object.get(field)) {
                    if seen.insert(call_id.clone()) {
                        ids.push(call_id);
                    }
                }
            }
        }
    });
    ids
}

pub(super) fn unpaired_tool_output_ids(value: &Value) -> Vec<String> {
    let calls: HashSet<_> = response_tool_call_ids(value).into_iter().collect();
    tool_call_output_ids(value)
        .into_iter()
        .filter(|id| !calls.contains(id))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyResponsesCallFamily {
    Function,
    Custom,
    Tool,
    Mcp,
    Computer,
}

impl LegacyResponsesCallFamily {
    fn call_type(self) -> &'static str {
        match self {
            Self::Function => "function_call",
            Self::Custom => "custom_tool_call",
            Self::Tool => "tool_call",
            Self::Mcp => "mcp_tool_call",
            Self::Computer => "computer_call",
        }
    }

    fn output_type(self) -> &'static str {
        match self {
            Self::Function => "function_call_output",
            Self::Custom => "custom_tool_call_output",
            Self::Tool => "tool_call_output",
            Self::Mcp => "mcp_tool_call_output",
            Self::Computer => "computer_call_output",
        }
    }
}

fn legacy_responses_call_family(item_type: &str) -> Option<LegacyResponsesCallFamily> {
    match item_type {
        "function_call" | "function_call_output" => Some(LegacyResponsesCallFamily::Function),
        "custom_tool_call" | "custom_tool_call_output" => Some(LegacyResponsesCallFamily::Custom),
        "tool_call" | "tool_call_output" => Some(LegacyResponsesCallFamily::Tool),
        "mcp_tool_call" | "mcp_tool_call_output" => Some(LegacyResponsesCallFamily::Mcp),
        "computer_call" | "computer_call_output" => Some(LegacyResponsesCallFamily::Computer),
        _ => None,
    }
}

/// Removes one incomplete function or custom-tool call only after the upstream
/// explicitly reports that its output is missing. The error identity must
/// match the call's item ID or call ID when the provider supplies one.
pub(super) fn remove_unpaired_responses_tool_call(
    request: &mut Value,
    historical_item_count: usize,
    upstream_error: &[u8],
) -> bool {
    let Some((expected_type, error_id)) = missing_responses_tool_call_identity(upstream_error)
    else {
        return false;
    };
    let Some(input) = request.get("input").and_then(Value::as_array) else {
        return false;
    };

    let mut incomplete = Vec::new();
    for (index, item) in input.iter().enumerate() {
        let Some(call_type) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let output_type = match call_type {
            "function_call" => "function_call_output",
            "custom_tool_call" => "custom_tool_call_output",
            _ => continue,
        };
        let call_id = item.get("call_id").and_then(Value::as_str);
        let item_id = item.get("id").and_then(Value::as_str);
        let has_output = input.iter().enumerate().skip(index + 1).any(|(_, output)| {
            output.get("type").and_then(Value::as_str) == Some(output_type)
                && output
                    .get("call_id")
                    .and_then(Value::as_str)
                    .is_some_and(|output_id| {
                        Some(output_id) == call_id || Some(output_id) == item_id
                    })
        });
        if !has_output {
            incomplete.push((index, call_type, call_id, item_id));
        }
    }

    // Multiple pending calls make it unsafe to guess which one the provider
    // rejected. Repair only a single incomplete call across the replayed turn.
    if incomplete.len() != 1 {
        return false;
    }
    let (index, call_type, call_id, item_id) = incomplete[0];
    if index >= historical_item_count
        || call_type != expected_type
        || error_id
            .as_deref()
            .is_some_and(|error_id| Some(error_id) != call_id && Some(error_id) != item_id)
    {
        return false;
    }

    request["input"]
        .as_array_mut()
        .expect("validated Responses input")
        .remove(index);
    true
}

fn missing_responses_tool_call_identity(payload: &[u8]) -> Option<(&'static str, Option<String>)> {
    let text = String::from_utf8_lossy(payload);
    let normalized = text.to_ascii_lowercase();
    for (prefix, call_type) in [
        ("no tool output found for function call ", "function_call"),
        (
            "no tool output found for custom tool call ",
            "custom_tool_call",
        ),
        (
            "no tool output found for apply patch call ",
            "custom_tool_call",
        ),
    ] {
        let Some(start) = normalized.find(prefix) else {
            continue;
        };
        let suffix = text[start + prefix.len()..].trim_start_matches(['"', '\'', '`']);
        let id = suffix
            .split(|character: char| {
                character.is_whitespace()
                    || matches!(
                        character,
                        '"' | '\'' | '`' | ',' | ';' | ':' | ')' | ']' | '}' | '>'
                    )
            })
            .next()
            .unwrap_or("")
            .trim_end_matches('.')
            .trim();
        let id = (!id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
            .then(|| id.to_string());
        return Some((call_type, id));
    }
    normalized
        .contains("unanswered_function_call")
        .then_some(("function_call", None))
}

fn legacy_responses_call_id(item: &Value) -> Option<&str> {
    item.get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
}

fn next_legacy_responses_call_id(
    index: usize,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let base = format!("call_missing_{index}");
    if used.insert(base.clone()) {
        return base;
    }
    for suffix in 1..=MAX_LEGACY_RESPONSES_REPAIR_ITEMS {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    // The item bound makes this unreachable in practice. Keep a deterministic
    // fallback so the helper cannot spin if its limits change later.
    format!("call_missing_{index}_overflow")
}

#[derive(Clone, Debug)]
struct PendingLegacyResponsesCall {
    index: usize,
    id: Option<String>,
    item_id: Option<String>,
    name: Option<String>,
    namespace: Option<String>,
    family: LegacyResponsesCallFamily,
}

fn legacy_responses_output_can_stand_alone(item_type: &str, name: Option<&str>) -> bool {
    item_type == "function_call_output" && name.is_some()
}

/// Repairs historical Responses links only after an explicit upstream rejection.
///
/// A result must identify exactly one earlier call of the same kind. Its item
/// ID may identify that call, but the result must use the call's `call_id`.
/// Plan every change before applying it: ambiguity or an anonymous orphan must
/// never delete results, cross namespaces, or leave a partially repaired turn.
pub(super) fn repair_legacy_responses_call_ids(request: &mut Value) -> bool {
    let Some(input) = request.get("input").and_then(Value::as_array) else {
        return false;
    };
    if input.is_empty() || input.len() > MAX_LEGACY_RESPONSES_REPAIR_ITEMS {
        return false;
    }

    let relevant_count = input
        .iter()
        .filter(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .and_then(legacy_responses_call_family)
                .is_some()
        })
        .count();
    if relevant_count > MAX_LEGACY_RESPONSES_PENDING_CALLS {
        return false;
    }

    let mut used = std::collections::HashSet::with_capacity(relevant_count);
    for item in input.iter() {
        for field in ["call_id", "id"] {
            if let Some(id) = bounded_tool_call_id(item.get(field)) {
                used.insert(id);
            }
        }
    }

    let mut pending = Vec::<PendingLegacyResponsesCall>::with_capacity(relevant_count);
    let mut assigned = HashSet::new();
    let mut edits = Vec::new();

    for (index, item) in input.iter().enumerate() {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(item_type) = object
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let Some(family) = legacy_responses_call_family(&item_type) else {
            continue;
        };
        if ["call_id", "id", "name", "namespace"].iter().any(|field| {
            object.get(*field).is_some_and(|value| {
                !value.is_null()
                    && value.as_str().is_none_or(|value| {
                        value != value.trim() || value.len() > MAX_LEGACY_RESPONSES_NAME_CHARS
                    })
            })
        }) {
            return false;
        }
        let is_call = item_type == family.call_type();
        let is_output = item_type == family.output_type();
        if !is_call && !is_output {
            continue;
        }

        let existing_id = legacy_responses_call_id(item).map(str::to_owned);
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| {
                !value.is_empty() && value.chars().count() <= MAX_LEGACY_RESPONSES_NAME_CHARS
            })
            .map(str::to_string);
        let namespace = object
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_owned);

        if is_call {
            if existing_id
                .as_ref()
                .is_some_and(|id| !assigned.insert(id.clone()))
            {
                return false;
            }
            pending.push(PendingLegacyResponsesCall {
                index,
                id: existing_id,
                item_id: bounded_tool_call_id(object.get("id")),
                name,
                namespace,
                family,
            });
            continue;
        }

        let mut matches = pending.iter().enumerate().filter(|(_, call)| {
            call.family == family
                && name
                    .as_ref()
                    .is_none_or(|name| call.name.as_ref() == Some(name))
                && namespace
                    .as_ref()
                    .is_none_or(|namespace| call.namespace.as_ref() == Some(namespace))
                && existing_id.as_ref().is_none_or(|id| {
                    call.id.as_ref() == Some(id) || call.item_id.as_ref() == Some(id)
                })
        });
        let position = matches.next().map(|(position, _)| position);
        if matches.next().is_some() {
            return false;
        }
        let Some(position) = position else {
            if existing_id.is_some()
                || legacy_responses_output_can_stand_alone(&item_type, name.as_deref())
            {
                continue;
            }
            return false;
        };
        let call = pending.remove(position);
        let call_id = call
            .id
            .clone()
            .or_else(|| existing_id.clone())
            .or(call.item_id)
            .unwrap_or_else(|| next_legacy_responses_call_id(call.index, &mut used));
        if call.id.is_none() {
            if !assigned.insert(call_id.clone()) {
                return false;
            }
            edits.push((call.index, call_id.clone()));
        }
        if existing_id.as_ref() != Some(&call_id) {
            edits.push((index, call_id));
        }
    }

    if !pending.is_empty() || edits.is_empty() {
        return false;
    }
    let input = request["input"].as_array_mut().expect("validated input");
    for (index, call_id) in edits {
        input[index]["call_id"] = Value::String(call_id);
    }
    true
}

fn visit_response_items(value: &Value, inspect: &mut impl FnMut(&serde_json::Map<String, Value>)) {
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| visit_response_items(item, inspect)),
        Value::Object(object) => {
            let kind = object.get("type").and_then(Value::as_str);
            if !object.contains_key("role")
                && kind.is_none_or(|kind| kind == "response" || kind.starts_with("response."))
            {
                // Only protocol envelopes contain history. Tool schemas,
                // arguments and result content are data, even with call IDs.
                for field in ["input", "output", "response", "item"] {
                    if let Some(item) = object.get(field) {
                        visit_response_items(item, inspect);
                    }
                }
            } else {
                inspect(object);
            }
        }
        _ => {}
    }
}

fn bounded_tool_call_id(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && value.len() <= 256).then(|| value.to_string())
}

pub(super) fn tool_use_diagnostics(value: &Value) -> ToolUseDiagnostics {
    let stats = crate::tool_policy::catalog_stats(value);
    ToolUseDiagnostics {
        client_tool_count: stats.count,
        client_schema_bytes: Some(stats.bytes),
        tool_choice: tool_choice_mode(value),
        ..ToolUseDiagnostics::default()
    }
}

pub(super) fn is_deferred_tool_search_compatibility_error(
    status: StatusCode,
    details: &crate::usage::UpstreamErrorDetails,
) -> bool {
    if !status.is_client_error() {
        return false;
    }
    let text = format!(
        "{} {} {}",
        details.code.as_deref().unwrap_or_default(),
        details.error_type.as_deref().unwrap_or_default(),
        details.message.as_deref().unwrap_or_default(),
    )
    .to_ascii_lowercase();
    text.contains("tool_search")
        || text.contains("tool search")
        || text.contains("defer_loading")
        || text.contains("deferred tool")
        || (text.contains("unsupported") && text.contains("tool"))
}

/// Internal request extension, never a wire header or persisted request body.
#[derive(Clone)]
pub(in crate::gateway) struct RequestToolPolicy {
    policy: crate::ToolPolicy,
    configured_mode: crate::ToolPolicyMode,
    policy_fallback: bool,
    deferred_disabled: bool,
    deferred_applied: bool,
    pub(in crate::gateway) diagnostics: ToolUseDiagnostics,
}

impl RequestToolPolicy {
    pub(in crate::gateway) fn new(runtime: &GatewayRuntime, request: &Value) -> Self {
        let policy = runtime.tool_policy();
        Self {
            configured_mode: policy.mode,
            policy,
            policy_fallback: false,
            deferred_disabled: false,
            deferred_applied: false,
            diagnostics: tool_use_diagnostics(request),
        }
    }

    pub(in crate::gateway) fn apply(&mut self, request: &mut Value) -> Result<(), &'static str> {
        self.apply_value(request, false)
    }

    pub(in crate::gateway) fn apply_value(
        &mut self,
        request: &mut Value,
        allow_deferred_tool_search: bool,
    ) -> Result<(), &'static str> {
        let result = crate::tool_policy::apply_tool_policy(request, &self.policy)?;
        let deferred = allow_deferred_tool_search
            && !self.deferred_disabled
            && matches!(
                self.diagnostics.tool_choice,
                crate::ToolChoiceMode::Auto | crate::ToolChoiceMode::Unspecified
            )
            && crate::tool_policy::enable_deferred_tool_search(request, &self.policy);
        self.deferred_applied |= deferred;
        self.record(result, deferred, request);
        Ok(())
    }

    pub(in crate::gateway) fn apply_adapter(
        &mut self,
        request: &mut crate::PreparedAdapterRequest,
        allow_deferred_tool_search: bool,
    ) -> Result<(), &'static str> {
        let result = request.apply_tool_policy(&self.policy)?;
        let deferred = allow_deferred_tool_search
            && !self.deferred_disabled
            && matches!(
                self.diagnostics.tool_choice,
                crate::ToolChoiceMode::Auto | crate::ToolChoiceMode::Unspecified
            )
            && crate::tool_policy::enable_deferred_tool_search(
                request.upstream_body_mut(),
                &self.policy,
            );
        self.deferred_applied |= deferred;
        self.record(result, deferred, request.upstream_body());
        Ok(())
    }

    fn record(
        &mut self,
        mut result: crate::tool_policy::ToolPolicyResult,
        deferred: bool,
        request: &Value,
    ) {
        if deferred {
            // Include the provider control tool and defer flags in the
            // serialized-catalog diagnostic for this attempt. These are wire
            // bytes, not a claim about model-context tokens.
            result.after = crate::tool_policy::catalog_stats(request);
            result.outcome = crate::ToolPolicyOutcome::Deferred;
        }
        // Keep the configured mode visible on every attempt.
        self.diagnostics.policy_mode = Some(self.configured_mode);
        self.diagnostics.policy_fallback = self.policy_fallback;
        self.diagnostics.deferred_tool_search = deferred;
        // Capture the exact post-policy Value that will be serialized. Do not
        // deserialize the entire conversation again just to count its catalog.
        // Usage rows describe one attempt, not the maximum across other routes.
        self.diagnostics.forwarded_tool_count = result.after.count;
        self.diagnostics.forwarded_schema_bytes = Some(result.after.bytes);
        self.diagnostics.filtered_tool_count =
            result.before.count.saturating_sub(result.after.count);
        self.diagnostics.policy_outcome = Some(result.outcome);
    }

    /// Allow one compatibility retry for a deferred request.
    pub(in crate::gateway) fn prepare_deferred_fallback(&mut self) -> bool {
        if self.deferred_applied && !self.deferred_disabled {
            self.deferred_disabled = true;
            self.policy_fallback = true;
            self.diagnostics.policy_fallback = true;
            return true;
        }
        false
    }
}

fn tool_choice_mode(value: &Value) -> ToolChoiceMode {
    let choice = value.get("tool_choice").or_else(|| {
        value
            .get("response")
            .and_then(|response| response.get("tool_choice"))
    });
    match choice {
        None => ToolChoiceMode::Unspecified,
        Some(Value::String(value)) => tool_choice_mode_from_type(value),
        Some(Value::Object(object)) => object
            .get("type")
            .and_then(Value::as_str)
            .map_or(ToolChoiceMode::Specific, tool_choice_mode_from_type),
        Some(_) => ToolChoiceMode::Unspecified,
    }
}

fn tool_choice_mode_from_type(value: &str) -> ToolChoiceMode {
    match value.to_ascii_lowercase().as_str() {
        "auto" => ToolChoiceMode::Auto,
        "required" | "any" => ToolChoiceMode::Required,
        "none" => ToolChoiceMode::None,
        "allowed_tools" => ToolChoiceMode::AllowedTools,
        _ => ToolChoiceMode::Specific,
    }
}

pub(super) fn candidate_protocols(wire_api: WireApi) -> &'static [WireApi] {
    match wire_api {
        WireApi::Responses => &[WireApi::Responses],
        WireApi::ChatCompletions => &[WireApi::ChatCompletions],
        WireApi::Messages => &[WireApi::Messages],
        WireApi::Gemini => &[WireApi::Gemini],
    }
}

pub(super) fn chat_request_is_text_or_image_only(value: &Value) -> bool {
    let Some(request) = value.as_object() else {
        return false;
    };
    if request.contains_key("audio") {
        return false;
    }
    if let Some(modalities) = request.get("modalities") {
        let Some(modalities) = modalities.as_array() else {
            return false;
        };
        if modalities
            .iter()
            .any(|modality| modality.as_str() != Some("text"))
        {
            return false;
        }
    }
    request
        .get("messages")
        .and_then(Value::as_array)
        .is_none_or(|messages| messages.iter().all(chat_message_is_text_or_image_only))
}

fn chat_message_is_text_or_image_only(message: &Value) -> bool {
    let Some(message) = message.as_object() else {
        return false;
    };
    match message.get("content") {
        None | Some(Value::Null) | Some(Value::String(_)) => true,
        Some(Value::Array(parts)) => parts.iter().all(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("text" | "image_url")
            )
        }),
        Some(_) => false,
    }
}

pub(super) fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("relay-{timestamp}-{sequence}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DefaultServiceTier, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource,
        RuntimeLocalKey, RuntimeSource,
    };
    use axum::http::{HeaderMap, HeaderValue};

    fn automatic_tool_policy_test_runtime() -> GatewayRuntime {
        let runtime = capability_test_runtime(&["synthetic"], GatewayRuntimeOptions::default());
        runtime
            .set_tool_policy(crate::ToolPolicy {
                mode: crate::ToolPolicyMode::Automatic,
            })
            .unwrap();
        runtime
    }

    fn two_function_tools() -> Value {
        json!({
            "tools": [
                {"type":"function","name":"lookup"},
                {"type":"function","name":"update"}
            ]
        })
    }

    #[test]
    fn tool_policy_snapshot_survives_hot_updates_and_retry_clones() {
        let runtime = capability_test_runtime(&["synthetic"], GatewayRuntimeOptions::default());
        runtime
            .set_tool_policy(crate::ToolPolicy {
                mode: crate::ToolPolicyMode::Automatic,
            })
            .unwrap();
        let original =
            json!({"tools":[{"type":"function","name":"keep"},{"type":"function","name":"drop"}]});
        let snapshot = RequestToolPolicy::new(&runtime, &original);
        runtime
            .set_tool_policy(crate::ToolPolicy::default())
            .unwrap();
        for mut attempt in [snapshot.clone(), snapshot] {
            let mut body = original.clone();
            attempt.apply(&mut body).unwrap();
            assert_eq!(body["tools"].as_array().unwrap().len(), 2);
            assert_eq!(attempt.diagnostics.client_tool_count, 2);
            assert_eq!(attempt.diagnostics.filtered_tool_count, 0);
            attempt.apply(&mut body).unwrap();
            assert_eq!(attempt.diagnostics.filtered_tool_count, 0);
            assert_eq!(
                attempt.diagnostics.policy_outcome,
                Some(crate::ToolPolicyOutcome::Unchanged)
            );
        }
        let mut next = original.clone();
        RequestToolPolicy::new(&runtime, &original)
            .apply(&mut next)
            .unwrap();
        assert_eq!(next, original);
    }

    #[test]
    fn direct_native_responses_policy_defers_then_restores_the_full_catalog() {
        let runtime = automatic_tool_policy_test_runtime();
        let original = two_function_tools();
        let mut policy = RequestToolPolicy::new(&runtime, &original);

        let mut deferred = original.clone();
        policy.apply_value(&mut deferred, true).unwrap();
        assert_eq!(deferred["tools"].as_array().unwrap().len(), 3);
        assert_eq!(deferred["tools"][0]["defer_loading"], true);
        assert_eq!(deferred["tools"][2], json!({"type":"tool_search"}));
        assert!(policy.diagnostics.deferred_tool_search);
        assert_eq!(
            policy.diagnostics.policy_outcome,
            Some(crate::ToolPolicyOutcome::Deferred)
        );

        assert!(policy.prepare_deferred_fallback());
        let mut fallback = original.clone();
        policy.apply_value(&mut fallback, true).unwrap();
        assert_eq!(fallback, original);
        assert!(!policy.diagnostics.deferred_tool_search);
        assert!(policy.diagnostics.policy_fallback);
        assert!(!policy.prepare_deferred_fallback());
    }

    #[test]
    fn direct_non_responses_account_endpoint_keeps_the_catalog_unchanged() {
        let runtime = automatic_tool_policy_test_runtime();
        let original = two_function_tools();
        let mut policy = RequestToolPolicy::new(&runtime, &original);
        let mut body = original.clone();
        policy.apply_value(&mut body, false).unwrap();
        assert_eq!(body, original);
        assert!(!policy.diagnostics.deferred_tool_search);
    }

    #[test]
    fn background_classifier_requires_explicit_codex_marker() {
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        assert_eq!(
            codex_background_request_kind(
                &headers,
                &json!({"model":"gpt-5.6-luna","reasoning":{"effort":"low"}})
            ),
            None
        );
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"request_type":"task_title"}"#),
        );
        assert_eq!(
            codex_background_request_kind(&headers, &json!({"model":"gpt-5.6-luna"})),
            Some(CODEX_TASK_TITLE)
        );
    }

    #[test]
    fn background_classifier_accepts_exact_internal_prompt_prefixes() {
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        headers.insert("x-openai-subagent", HeaderValue::from_static("true"));
        assert_eq!(
            codex_background_request_kind(
                &headers,
                &json!({"input":[{"content":[{"text":"Summarize the activity for this task"}]}]})
            ),
            Some(CODEX_ACTIVITY_SUMMARY)
        );
        assert_eq!(
            codex_background_request_kind(
                &headers,
                &json!({"instructions":"Generate a concise title for this task"})
            ),
            Some(CODEX_TASK_TITLE)
        );
    }

    #[test]
    fn background_classifier_does_not_use_prompt_text_without_marker() {
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        assert_eq!(
            codex_background_request_kind(
                &headers,
                &json!({"instructions":"Summarize the activity for this task, then answer me"})
            ),
            None
        );
    }

    #[test]
    fn tool_diagnostics_count_codex_tool_definitions_without_names() {
        let request = json!({
            "tools": [
                {"type": "function", "name": "read_private_file"},
                {"type": "namespace", "name": "collaboration", "tools": [
                    {"type": "function", "name": "spawn_agent"},
                    {"type": "function", "name": "wait_agent"}
                ]}
            ],
            "input": [{
                "type": "additional_tools",
                "tools": [{"type": "custom", "name": "apply_patch"}]
            }],
            "response": {
                "tools": [{"type": "function", "name": "hidden_function"}]
            },
            "tool_choice": {"type": "allowed_tools", "tools": []}
        });

        let diagnostics = tool_use_diagnostics(&request);
        let runtime = capability_test_runtime(&["synthetic"], GatewayRuntimeOptions::default());
        let mut policy = RequestToolPolicy::new(&runtime, &request);
        policy.apply(&mut request.clone()).unwrap();
        let forwarded = policy.diagnostics;

        assert_eq!(diagnostics.client_tool_count, 5);
        assert_eq!(diagnostics.tool_choice, ToolChoiceMode::AllowedTools);
        assert_eq!(forwarded.forwarded_tool_count, 5);
        assert!(!serde_json::to_string(&forwarded)
            .unwrap()
            .contains("read_private_file"));
    }

    #[test]
    fn service_tier_defaults_inject_speed_without_overriding_client_choice() {
        let mut request = json!({});
        apply_default_service_tier_if_missing(&mut request, DefaultServiceTier::Fast);
        assert_eq!(request["service_tier"], "priority");
        assert_eq!(request_service_tier(&request), DefaultServiceTier::Fast);

        let mut ultrafast = json!({});
        apply_default_service_tier_if_missing(&mut ultrafast, DefaultServiceTier::Ultrafast);
        assert_eq!(ultrafast["service_tier"], "ultrafast");
        assert_eq!(
            request_service_tier(&ultrafast),
            DefaultServiceTier::Ultrafast
        );

        let mut standard = json!({});
        apply_default_service_tier_if_missing(&mut standard, DefaultServiceTier::Standard);
        assert!(standard.get("service_tier").is_none());

        let mut client_selected = json!({"service_tier": "flex"});
        apply_default_service_tier_if_missing(&mut client_selected, DefaultServiceTier::Fast);
        assert_eq!(client_selected["service_tier"], "flex");

        assert_eq!(
            request_service_tier(&json!({"service_tier": "priority"})),
            DefaultServiceTier::Fast
        );
        assert_eq!(
            request_service_tier(&json!({"service_tier": "fast"})),
            DefaultServiceTier::Fast
        );
        assert_eq!(
            request_service_tier(&json!({"service_tier": "ultrafast"})),
            DefaultServiceTier::Ultrafast
        );
        for tier in [None, Some("standard"), Some("default"), Some("flex")] {
            let request = tier.map_or_else(|| json!({}), |tier| json!({"service_tier": tier}));
            assert_eq!(
                request_service_tier(&request),
                DefaultServiceTier::Standard,
                "{tier:?} must remain a non-fast client tier"
            );
        }
    }

    #[test]
    fn native_account_reasoning_and_speed_selections_are_opaque() {
        let mut request = json!({
            "model": "gpt-5.6-terra",
            "service_tier": "flex",
            "reasoning": {
                "effort": "ultra",
                "summary": "detailed",
                "context": "client_selected"
            }
        });

        normalize_account_request(request.as_object_mut().unwrap(), false);

        assert_eq!(request["service_tier"], "flex");
        assert_eq!(request["reasoning"]["effort"], "ultra");
        assert_eq!(request["reasoning"]["summary"], "detailed");
        assert_eq!(request["reasoning"]["context"], "client_selected");
    }

    #[test]
    fn responses_lite_keeps_provider_owned_tools_and_choices_opaque_and_serializes_tools() {
        let mut request = json!({
            "model": "gpt-lite",
            "tools": [
                {"type": "function", "name": "lookup"},
                {"type": "namespace", "name": "collaboration", "tools": [
                    {"name": "spawn_agent"}
                ]},
                {"type": "web_search"},
                {"type": "future_client_tool", "name": "future_tool"}
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "required",
                "tools": [
                    {"type": "function", "name": "lookup"},
                    {"type": "web_search"}
                ]
            },
            "input": [
                {"type": "additional_tools", "tools": [
                    {"type": "custom", "name": "patch"},
                    {"type": "image_generation"}
                ]},
                {"role": "user", "content": "hello"}
            ]
        });
        let original_tools = request["tools"].clone();
        let original_choice = request["tool_choice"].clone();
        let original_input = request["input"].clone();

        normalize_account_request(request.as_object_mut().unwrap(), true);

        assert_eq!(request["tools"], original_tools);
        assert_eq!(request["tool_choice"], original_choice);
        assert_eq!(request["input"], original_input);
        assert_eq!(request["reasoning"]["context"], "all_turns");
        assert_eq!(request["parallel_tool_calls"], false);

        let mut no_tools = json!({"model": "gpt-lite"});
        normalize_account_request(no_tools.as_object_mut().unwrap(), true);
        assert_eq!(no_tools["parallel_tool_calls"], false);
    }

    #[test]
    fn responses_lite_forces_all_turns_reasoning_context_without_losing_effort() {
        let mut request = json!({
            "reasoning": {"effort": "high", "summary": "detailed"}
        });

        normalize_account_request(request.as_object_mut().unwrap(), true);

        assert_eq!(request["reasoning"]["context"], "all_turns");
        assert_eq!(request["reasoning"]["effort"], "high");
        assert_eq!(request["reasoning"]["summary"], "detailed");

        let mut malformed = json!({"reasoning": null});
        normalize_account_request(malformed.as_object_mut().unwrap(), true);
        assert_eq!(malformed["reasoning"], json!({"context": "all_turns"}));
    }

    #[test]
    fn tool_output_detection_covers_all_client_tool_result_shapes() {
        for output in [
            json!({"type": "function_call_output", "call_id": "call_function"}),
            json!({"type": "custom_tool_call_output", "call_id": "call_custom"}),
            json!({"type": "tool_search_output", "call_id": "call_search"}),
            json!({"type": "computer_call_output", "call_id": "call_future"}),
        ] {
            assert!(contains_tool_call_output(&json!({"input": [output]})));
        }
        assert!(!contains_tool_call_output(&json!({
            "input": [{"type": "custom_tool_call", "call_id": "call_custom"}]
        })));
        assert!(!contains_tool_call_output(&json!({
            "tools": [{"type":"function", "name":"inspect", "parameters":{
                "type":"object", "examples":[{"type":"function_call_output", "call_id":"example"}]
            }}],
            "input": "Inspect the provided example"
        })));
    }

    #[test]
    fn stale_custom_tool_recovery_removes_only_the_reported_historical_call() {
        let error =
            br#"{"error":{"message":"No tool output found for custom tool call ctc_stale."}}"#;
        for call in [
            json!({"type":"custom_tool_call","id":"item_stale","call_id":"ctc_stale","name":"patch","input":"{}"}),
            json!({"type":"custom_tool_call","id":"ctc_stale","name":"patch","input":"{}"}),
        ] {
            let mut request = json!({"input":[
                {"type":"message","role":"user","content":"start"},
                {"type":"function_call","call_id":"fc_done","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","call_id":"fc_done","output":"keep this result"},
                call,
                {"type":"message","role":"user","content":"continue"}
            ]});

            assert!(remove_unpaired_responses_tool_call(&mut request, 4, error));
            assert_eq!(request["input"].as_array().unwrap().len(), 4);
            assert_eq!(request["input"][0]["content"], "start");
            assert_eq!(request["input"][1]["call_id"], "fc_done");
            assert_eq!(request["input"][2]["output"], "keep this result");
            assert_eq!(request["input"][3]["content"], "continue");
        }
    }

    #[test]
    fn stale_tool_recovery_leaves_ambiguous_mismatched_and_current_calls_untouched() {
        let error =
            br#"{"error":{"message":"No tool output found for custom tool call ctc_stale."}}"#;
        for (input, historical_item_count) in [
            (
                json!([
                    {"type":"custom_tool_call","call_id":"ctc_stale","name":"patch","input":"{}"},
                    {"type":"custom_tool_call","call_id":"ctc_other","name":"patch","input":"{}"}
                ]),
                2,
            ),
            (
                json!([
                    {"type":"custom_tool_call","call_id":"ctc_other","name":"patch","input":"{}"}
                ]),
                1,
            ),
            (
                json!([
                    {"type":"message","role":"user","content":"current"},
                    {"type":"custom_tool_call","call_id":"ctc_stale","name":"patch","input":"{}"}
                ]),
                1,
            ),
        ] {
            let mut request = json!({"input":input});
            let original = request.clone();
            assert!(!remove_unpaired_responses_tool_call(
                &mut request,
                historical_item_count,
                error,
            ));
            assert_eq!(request, original);
        }
    }

    #[test]
    fn tool_affinity_extracts_all_bounded_output_ids() {
        let request = json!({
            "previous_response_id": "resp_old",
            "input": [
                {"type": "custom_tool_call_output", "call_id": "ctc_old"},
                {"type": "custom_tool_call_output", "call_id": "ctc_old"},
                {"type": "function_call_output", "call_id": "function"},
                {"type": "custom_tool_call_output", "call_id": "   "}
            ]
        });
        assert_eq!(tool_call_output_ids(&request), vec!["ctc_old", "function"]);

        let response = json!({
            "id": "resp_old",
            "output": [{
                "type": "custom_tool_call",
                "id": "ctc_item",
                "call_id": "call_custom",
                "input": "Get-ChildItem"
            }]
        });
        for envelope in [
            response.clone(),
            json!({"type":"response.completed", "response":response}),
            json!({"type":"response.output_item.done", "item":response["output"][0]}),
        ] {
            assert_eq!(
                response_tool_call_ids(&envelope),
                vec!["call_custom", "ctc_item"]
            );
        }

        let paired = json!({
            "input": [
                {"type": "computer_call", "id": "computer_item", "call_id": "computer_call"},
                {"type": "computer_call_output", "call_id": "computer_call"}
            ]
        });
        assert_eq!(
            response_tool_call_ids(&paired),
            vec!["computer_call", "computer_item"]
        );
        assert_eq!(tool_call_output_ids(&paired), vec!["computer_call"]);
    }

    #[test]
    fn legacy_responses_call_id_repair_preserves_items_and_tool_namespaces() {
        let mut request = json!({
            "input": [
                {"type": "message", "role": "user", "content": "continue"},
                {"type": "function_call", "id": "fc_existing", "name": "lookup", "namespace": "functions", "arguments": "{}"},
                {"type": "function_call_output", "name": "lookup", "output": "lookup result"},
                {"type": "custom_tool_call", "name": "patch", "namespace": "tools", "input": "{}"},
                {"type": "custom_tool_call_output", "name": "patch", "output": "patch result"},
                {"type": "function_call_output", "name": "heartbeat", "output": "keep standalone"}
            ]
        });

        assert!(repair_legacy_responses_call_ids(&mut request));
        let input = request["input"].as_array().expect("input array");
        assert_eq!(input.len(), 6);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "fc_existing");
        assert_eq!(input[1]["id"], "fc_existing");
        assert_eq!(input[1]["namespace"], "functions");
        assert_eq!(input[2]["call_id"], input[1]["call_id"]);
        assert!(input[3]["call_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("call_missing_")));
        assert_eq!(input[3]["namespace"], "tools");
        assert_eq!(input[4]["call_id"], input[3]["call_id"]);
        assert_eq!(input[5]["name"], "heartbeat");
        assert!(input[5].get("call_id").is_none());
    }

    #[test]
    fn tool_link_repair_keeps_explicit_ids_and_resolves_item_id_references() {
        for kind in ["function_call", "custom_tool_call"] {
            for call_id in [None, Some("call_stable")] {
                let mut call = json!({"type":kind,"id":"item_legacy","name":"lookup"});
                if let Some(id) = call_id {
                    call["call_id"] = json!(id);
                }
                let mut request = json!({"input":[
                    call,
                    {"type":format!("{kind}_output"),"call_id":"item_legacy","output":"synthetic result"}
                ]});
                assert!(repair_legacy_responses_call_ids(&mut request));
                let expected = call_id.unwrap_or("item_legacy");
                assert_eq!(request["input"][0]["call_id"], expected);
                assert_eq!(request["input"][1]["call_id"], expected);
                assert_eq!(request["input"][0]["id"], "item_legacy");
                assert_eq!(request["input"][1]["output"], "synthetic result");
                assert!(!repair_legacy_responses_call_ids(&mut request));
            }
        }
    }

    #[test]
    fn tool_link_repair_is_atomic_for_orphans_ambiguous_and_mismatched_results() {
        let call = json!({"type":"function_call","name":"lookup","arguments":"{}"});
        for invalid in [
            json!([{ "type":"function_call_output","output":"orphan" }]),
            json!([call, {"type":"custom_tool_call_output","output":"wrong kind"}]),
            json!([call, {"type":"function_call_output","name":"different","output":"wrong name"}]),
            json!([call, {"type":"function_call_output","namespace":"different","output":"wrong namespace"}]),
            json!([call, call, {"type":"function_call_output","output":"ambiguous"}]),
            json!([{"type":"function_call","id":"alias","call_id":"canonical"},
                {"type":"function_call","id":"other","call_id":"alias"},
                {"type":"function_call_output","call_id":"alias","output":"ambiguous alias"}]),
        ] {
            let mut input = vec![
                call.clone(),
                json!({"type":"function_call_output","output":"paired"}),
            ];
            input.extend(invalid.as_array().unwrap().iter().cloned());
            let mut request = json!({"input": input});
            let original = request.clone();
            assert!(!repair_legacy_responses_call_ids(&mut request));
            assert_eq!(request, original);
        }
    }

    #[test]
    fn tool_link_repair_matches_parallel_results_by_namespace_without_reordering() {
        let mut request = json!({"input":[
            {"type":"function_call","name":"lookup","namespace":"first","arguments":"{}"},
            {"type":"function_call","name":"lookup","namespace":"second","arguments":"{}"},
            {"type":"function_call_output","name":"lookup","namespace":"second","output":"second result"},
            {"type":"function_call_output","name":"lookup","namespace":"first","output":"first result"}
        ]});
        assert!(repair_legacy_responses_call_ids(&mut request));
        assert_eq!(
            request["input"][0]["call_id"],
            request["input"][3]["call_id"]
        );
        assert_eq!(
            request["input"][1]["call_id"],
            request["input"][2]["call_id"]
        );
        assert_ne!(
            request["input"][0]["call_id"],
            request["input"][1]["call_id"]
        );
        assert_eq!(request["input"][2]["output"], "second result");
    }

    #[test]
    fn legacy_responses_call_id_repair_is_idempotent_and_preserves_valid_history() {
        let mut request = json!({
            "input": [
                {"type": "function_call", "call_id": "known", "name": "lookup", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "known", "output": "ok"},
                {"type": "custom_tool_call", "call_id": "custom", "name": "patch", "input": "{}"},
                {"type": "custom_tool_call_output", "call_id": "custom", "output": "done"}
            ]
        });
        let original = request.clone();

        assert!(!repair_legacy_responses_call_ids(&mut request));
        assert_eq!(request, original);

        let mut legacy = json!({
            "input": [
                {"type": "function_call", "name": "lookup", "arguments": "{}"},
                {"type": "function_call_output", "output": "ok"}
            ]
        });
        assert!(repair_legacy_responses_call_ids(&mut legacy));
        let repaired = legacy.clone();
        assert!(!repair_legacy_responses_call_ids(&mut legacy));
        assert_eq!(legacy, repaired);
    }

    #[test]
    fn legacy_responses_call_id_repair_fails_closed_at_history_bounds() {
        let mut too_many_items =
            json!({"input": vec![json!({"role": "user"}); MAX_LEGACY_RESPONSES_REPAIR_ITEMS + 1]});
        let original = too_many_items.clone();
        assert!(!repair_legacy_responses_call_ids(&mut too_many_items));
        assert_eq!(too_many_items, original);

        let mut too_many_calls = json!({
            "input": (0..=MAX_LEGACY_RESPONSES_PENDING_CALLS)
                .map(|index| json!({"type": "function_call", "name": format!("tool-{index}")}))
                .collect::<Vec<_>>()
        });
        let original = too_many_calls.clone();
        assert!(!repair_legacy_responses_call_ids(&mut too_many_calls));
        assert_eq!(too_many_calls, original);
    }

    #[test]
    fn account_requests_normalize_non_array_input() {
        for (input, expected) in [
            (
                json!("hello"),
                json!([{"role":"user","content":[{"type":"input_text","text":"hello"}]}]),
            ),
            (json!("  "), json!([])),
            (
                json!({"role":"user","content":"hello"}),
                json!([{"role":"user","content":"hello"}]),
            ),
        ] {
            let mut request = json!({"input": input});
            normalize_account_request(request.as_object_mut().unwrap(), false);
            assert_eq!(request["input"], expected);
        }
    }

    #[test]
    fn account_requests_drop_unusable_reasoning_ids_when_history_is_not_stored() {
        let mut request = json!({
            "store": true,
            "input": [
                {"id": "rs_orphan", "type": "reasoning", "summary": []},
                {"id": "rs_null", "type": "reasoning", "encrypted_content": null, "summary": []},
                {"id": "rs_valid", "type": "reasoning", "encrypted_content": "signed-content", "summary": []},
                {"id": "msg_1", "type": "message", "role": "user", "content": "hello"}
            ]
        });

        normalize_account_request(request.as_object_mut().unwrap(), false);

        assert_eq!(request["store"], false);
        assert!(request.pointer("/input/0/id").is_none());
        assert!(request.pointer("/input/1/id").is_none());
        assert!(request.pointer("/input/1/encrypted_content").is_none());
        assert_eq!(request.pointer("/input/2/id").unwrap(), "rs_valid");
        assert_eq!(request.pointer("/input/3/id").unwrap(), "msg_1");
    }

    #[test]
    fn api_sources_generate_strict_codex_models_without_hidden_or_media_rows() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec![
                    "vendor/claude-opus-4-8".into(),
                    "gpt-image-2".into(),
                    "hidden-code".into(),
                    "disabled-code".into(),
                ],
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "secret".into(),
            })],
            GatewayRuntimeOptions {
                hidden_models: vec!["hidden-code".into()],
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let upstream = json!({"models": [
            {"slug": "gpt-image-2", "supported_in_api": true},
            {"slug": "disabled-code", "supported_in_api": false}
        ]});

        let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
            .expect("coding model catalog");
        let models = response["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        for model in models {
            assert!(codex_catalog_entry_is_compatible(model));
        }
        let claude = models
            .iter()
            .find(|model| model["slug"] == crate::codex_model_alias("vendor/claude-opus-4-8"))
            .expect("routed Claude model");
        assert_eq!(claude["display_name"], "Claude Opus 4.8");
        assert_eq!(claude["supported_reasoning_levels"], json!([]));
        assert!(claude.get("default_reasoning_level").is_none());
        assert_eq!(claude["input_modalities"], json!(["text", "image"]));
        assert!(models
            .iter()
            .any(|model| { model["slug"] == crate::codex_model_alias("disabled-code") }));
    }

    fn capability_test_runtime(models: &[&str], options: GatewayRuntimeOptions) -> GatewayRuntime {
        GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: models.iter().map(|id| (*id).to_string()).collect(),
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "secret".into(),
            })],
            options,
            Arc::new(|_| {}),
        )
        .unwrap()
    }

    #[test]
    fn api_gpt_picker_ids_stay_native_without_account_cards_or_extra_models() {
        let runtime = capability_test_runtime(
            &[
                "gpt-6-astra",
                "gpt-5.6-sol",
                "gpt-future",
                "provider/gpt-6-astra",
                "claude-future",
                "gpt-hidden",
            ],
            GatewayRuntimeOptions {
                hidden_models: vec!["gpt-hidden".into()],
                ..GatewayRuntimeOptions::default()
            },
        );
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let mut unrelated = crate::routed_codex_catalog_entry(None, "gpt-not-in-pool", 1_000, None);
        unrelated["slug"] = json!("gpt-not-in-pool");
        unrelated["supports_parallel_tool_calls"] = json!(true);
        unrelated["use_responses_lite"] = json!(true);
        let response = build_codex_models_response(
            &runtime,
            &key,
            &visible,
            Some(&json!({"models": [unrelated]})),
        )
        .unwrap();
        let models = response["models"].as_array().unwrap();
        assert_eq!(models.len(), 5);
        for id in ["gpt-6-astra", "gpt-5.6-sol", "gpt-future"] {
            let model = models.iter().find(|model| model["slug"] == id).unwrap();
            assert!(codex_catalog_entry_is_compatible(model));
            assert_eq!(model["comp_hash"], crate::CODEX_RELAY_CATALOG_HASH);
            assert_eq!(model["supports_parallel_tool_calls"], true);
            assert_eq!(model["supported_reasoning_levels"], json!([]));
            assert!(model.get("use_responses_lite").is_none());
        }
        for id in ["provider/gpt-6-astra", "claude-future"] {
            assert!(models
                .iter()
                .any(|model| model["slug"] == crate::codex_model_alias(id)));
        }
    }

    #[test]
    fn known_model_uses_catalog_capabilities_without_overriding_codex_context() {
        use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
        let catalog = ModelMetadataCatalog::from_models_dev_json(
            r#"{
            "vendor/claude-fable-5": {"reasoning": true, "reasoning_effort_levels": ["low", "high"],
            "default_reasoning_effort": "high", "tool_call": true,
            "modalities": {"input": ["text"], "output": ["text"]}, "limit": {"context": 64000}}
        }"#,
        )
        .unwrap();
        let runtime = capability_test_runtime(
            &["vendor/claude-fable-5"],
            GatewayRuntimeOptions {
                model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(catalog)),
                ..GatewayRuntimeOptions::default()
            },
        );
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let response = build_codex_models_response(&runtime, &key, &visible, None).unwrap();
        let entry = &response["models"][0];
        assert_eq!(entry["input_modalities"], json!(["text"]));
        assert!(entry.get("context_window").is_none());
        assert_eq!(entry["supports_parallel_tool_calls"], true);
        assert_eq!(entry["default_reasoning_level"], "high");
        assert_eq!(
            entry["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(codex_catalog_entry_is_compatible(entry));
    }

    #[test]
    fn messages_bridge_does_not_invent_codex_ultra_from_max() {
        use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
        let model = "anthropic/claude-fable-5-1";
        let catalog = ModelMetadataCatalog::from_models_dev_json(
            r#"{
            "anthropic/claude-fable-5-1": {
                "reasoning": true,
                "reasoning_effort_levels": ["low", "medium", "high", "xhigh", "max"]
            }}"#,
        )
        .unwrap();
        let source = ProviderSource {
            id: "source".into(),
            name: "source".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "upstream-secret".into(),
            wire_api: WireApi::Responses,
            models: vec![model.into()],
        };
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource {
                protocol_config: crate::SourceProtocolConfig {
                    endpoint_hint: Some(WireApi::Messages),
                    ..Default::default()
                },
                ..RuntimeSource::unrestricted(source)
            }],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "secret".into(),
            })],
            GatewayRuntimeOptions {
                model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(catalog)),
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let response = build_codex_models_response(&runtime, &key, &visible, None).unwrap();

        assert_eq!(
            response["models"][0]["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|level| level["effort"].as_str())
                .collect::<Vec<_>>(),
            ["low", "medium", "high", "max"]
        );
        // The adapter can forward Max but cannot forward xhigh. Codex Ultra
        // is a client orchestration mode, not a synonym for this route's Max.
        assert_eq!(
            runtime.model_capabilities(model).reasoning_effort_levels,
            ["low", "medium", "high", "xhigh", "max"]
        );
    }

    #[test]
    fn manual_overrides_do_not_grant_unknown_reasoning_capabilities() {
        let runtime =
            capability_test_runtime(&["vendor/claude-fable-5"], GatewayRuntimeOptions::default());
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let response = build_codex_models_response(&runtime, &key, &visible, None)
            .expect("coding model catalog");
        let model = &response["models"][0];

        assert_eq!(
            model["slug"],
            crate::codex_model_alias("vendor/claude-fable-5")
        );
        assert!(model.get("default_reasoning_level").is_none());
        assert_eq!(model["supported_reasoning_levels"], json!([]));
        assert_eq!(model["supports_reasoning_summary_parameter"], false);
        assert_eq!(model["supports_reasoning_summaries"], false);
        assert_eq!(model["default_reasoning_summary"], "none");
        assert!(codex_catalog_entry_is_compatible(model));

        runtime
            .set_model_reasoning_allowed_levels(std::collections::BTreeMap::from([(
                "vendor/claude-fable-5".to_string(),
                vec!["ultra".to_string()],
            )]))
            .unwrap();
        let configured = build_codex_models_response(&runtime, &key, &visible, None)
            .expect("coding model catalog");
        let configured_model = &configured["models"][0];
        assert!(configured_model.get("default_reasoning_level").is_none());
        assert_eq!(configured_model["supported_reasoning_levels"], json!([]));

        runtime
            .set_model_reasoning_allowed_levels(std::collections::BTreeMap::new())
            .unwrap();
        let no_manual_selection = build_codex_models_response(&runtime, &key, &visible, None)
            .expect("coding model catalog");
        assert_eq!(
            no_manual_selection["models"][0]["supported_reasoning_levels"],
            json!([])
        );
    }

    #[test]
    fn codex_catalog_names_do_not_merge_routes_or_grant_native_transport() {
        use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

        let metadata = ModelMetadataCatalog::from_models_dev_json(
            r#"{
            "openai/future-model":{"name":"Future Name"},
            "alpha/shared":{"name":"Same Display Name"},
            "beta/shared":{"name":"Same Display Name"}
        }"#,
        )
        .unwrap();
        let runtime = capability_test_runtime(
            &[
                "openai/future-model",
                "alpha/shared",
                "beta/shared",
                "unknown-model",
            ],
            GatewayRuntimeOptions {
                model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(metadata)),
                ..GatewayRuntimeOptions::default()
            },
        );
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let response = build_codex_models_response(&runtime, &key, &visible, None).unwrap();
        let rows = response["models"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        for (row, id) in rows.iter().zip(&visible) {
            let expected = match id.as_str() {
                "openai/future-model" => "Future Name",
                "alpha/shared" | "beta/shared" => "Same Display Name",
                _ => "Unknown Model",
            };
            assert_eq!(row["display_name"], expected);
            assert_eq!(
                runtime
                    .resolve_configured_model(
                        &key,
                        row["slug"].as_str().unwrap(),
                        &[WireApi::Responses]
                    )
                    .as_ref(),
                Some(id)
            );
            assert_eq!(row["supports_parallel_tool_calls"], true);
            assert_eq!(row["supported_reasoning_levels"], json!([]));
            assert!(row.get("use_responses_lite").is_none());
            assert_eq!(
                row["service_tiers"].as_array().map(Vec::len),
                Some(if id == "openai/future-model" { 2 } else { 0 })
            );
        }
        assert_ne!(rows[1]["slug"], rows[2]["slug"]);
    }

    #[test]
    fn codex_catalog_uses_shared_image_defaults_for_unknown_models() {
        let runtime =
            capability_test_runtime(&["vendor/claude-fable-5"], GatewayRuntimeOptions::default());
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let response = build_codex_models_response(&runtime, &key, &visible, None)
            .expect("coding model catalog");

        assert_eq!(
            response["models"][0]["input_modalities"],
            json!(["text", "image"])
        );
        assert!(codex_catalog_entry_is_compatible(&response["models"][0]));
    }

    #[test]
    fn codex_catalog_uses_unique_priorities_and_shared_capability_defaults() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec![
                    "vendor/glm-5.2".into(),
                    "vendor/grok-4.5".into(),
                    "vendor/gemini-3.6-flash".into(),
                    "vendor/claude-opus-4-8".into(),
                    "gpt-5.4".into(),
                ],
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "secret".into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let upstream = json!({"models": [{
            "slug": "vendor/glm-5.2",
            "supported_in_api": true
        }, {
            "slug": "vendor/grok-4.5",
            "supported_in_api": true
        }, {
            "slug": "vendor/gemini-3.6-flash",
            "supported_in_api": true
        }, {
            "slug": "vendor/claude-opus-4-8",
            "supported_in_api": true
        }, {
            "slug": "gpt-5.4",
            "use_responses_lite": true,
            "supports_parallel_tool_calls": true
        }]});

        let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
            .expect("coding model catalog");
        let models = response["models"].as_array().unwrap();
        let priorities = models
            .iter()
            .filter_map(|model| model["priority"].as_u64())
            .collect::<Vec<_>>();
        let display_names = models
            .iter()
            .filter_map(|model| model["display_name"].as_str())
            .collect::<Vec<_>>();

        assert_eq!(priorities, [1_000, 1_001, 1_002, 1_003, 1_004]);
        assert_eq!(
            display_names,
            [
                "GLM 5.2",
                "Grok 4.5",
                "Gemini 3.6 Flash",
                "Claude Opus 4.8",
                "5.4",
            ]
        );
        assert!(models.iter().all(codex_catalog_entry_is_compatible));
        // A generic Responses source can reuse an OpenAI-looking model ID
        // without supporting Codex's native tool contract. Only account
        // manifests are authoritative for this capability.
        assert_eq!(models[0]["supports_parallel_tool_calls"], true);
    }

    #[test]
    fn mixed_upstream_and_fallback_catalog_rows_get_unique_priorities() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec![
                    "gpt-5.6-sol".into(),
                    "vendor/claude-opus".into(),
                    "vendor/grok".into(),
                ],
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "secret".into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let upstream = json!({
            "models": [
                {"slug": "gpt-5.6-sol", "priority": 1_000},
            ]
        });

        let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
            .expect("coding model catalog");
        let models = response["models"].as_array().unwrap();
        let priorities = models
            .iter()
            .map(|model| model["priority"].as_u64().expect("priority"))
            .collect::<Vec<_>>();

        assert_eq!(priorities, [1_000, 1_001, 1_002]);
        assert_eq!(
            models
                .iter()
                .map(|model| model["display_name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["5.6 Sol", "Claude Opus", "Grok"]
        );
    }

    #[test]
    fn provider_context_is_not_advertised_for_unknown_models() {
        let runtime = capability_test_runtime(&["gpt-5.4"], GatewayRuntimeOptions::default());
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let upstream = json!({"models": [{
            "slug": "gpt-5.4",
            "context_window": 128_000,
            "max_context_window": 128_000,
            "auto_compact_token_limit": 122_000,
            "effective_context_window_percent": 95
        }]});
        let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
            .expect("coding model catalog");
        let model = &response["models"][0];

        assert!(model.get("context_window").is_none());
        assert!(model.get("max_context_window").is_none());
        assert!(model.get("auto_compact_token_limit").is_none());
    }
}

mod account;
mod codex_models;
mod headers;
mod normalization;

#[cfg(test)]
use super::now_ms;
pub(super) use account::{account_endpoint_url, alpha_search, responses_compact, AccountEndpoint};
pub(super) use codex_models::models;
#[cfg(test)]
use codex_models::{
    build_codex_models_response, build_codex_models_response_with_source_capabilities,
    build_codex_models_response_with_source_reasoning,
};
pub(super) use headers::{
    apply_codex_routing_hint, client_context_fingerprint, codex_client_version,
    forwarded_bridge_gemini_headers, forwarded_bridge_messages_headers, forwarded_codex_headers,
    forwarded_messages_headers, is_managed_codex_client,
};
#[cfg(test)]
pub(super) use normalization::{apply_default_service_tier_if_missing, request_service_tier};
pub(super) use normalization::{
    normalize_account_request, normalize_compact_account_request, normalize_responses_lite_request,
    responses_lite_parallel_tool_calls_valid, try_recover_encrypted_content, ServiceTierPolicy,
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) const MAX_CLIENT_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

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

#[cfg(test)]
pub(super) fn requested_reasoning_effort(request: &Value, wire_api: WireApi) -> Option<String> {
    let effort = match wire_api {
        WireApi::Responses => request.pointer("/reasoning/effort"),
        WireApi::ChatCompletions => request.get("reasoning_effort"),
        WireApi::Messages => None,
        WireApi::Gemini => None,
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
            "invalid_request",
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
    (!model.is_empty()
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| (model.to_string(), stream))
}

pub(super) fn contains_tool_call_output(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_tool_call_output),
        Value::Object(object) => {
            object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "tool_search_output" || kind.ends_with("_call_output"))
                || object.values().any(contains_tool_call_output)
        }
        _ => false,
    }
}

/// Returns the stable ids carried by Responses tool outputs. These ids are
/// stateful when the matching call is not included in the same request: the
/// provider that emitted the call is then the only safe owner. Keep this
/// extraction bounded and do not retain the tool payload itself.
pub(super) fn tool_call_output_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    collect_tool_call_output_ids(value, &mut ids);
    ids
}

fn collect_tool_call_output_ids(value: &Value, ids: &mut Vec<String>) {
    if ids.len() >= 16 {
        return;
    }
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_tool_call_output_ids(item, ids)),
        Value::Object(object) => {
            let kind = object.get("type").and_then(Value::as_str);
            if kind
                .is_some_and(|kind| kind == "tool_search_output" || kind.ends_with("_call_output"))
            {
                if let Some(call_id) = bounded_tool_call_id(object.get("call_id")) {
                    if !ids.iter().any(|known| known == &call_id) {
                        ids.push(call_id);
                    }
                }
            }
            object
                .values()
                .for_each(|item| collect_tool_call_output_ids(item, ids));
        }
        _ => {}
    }
}

/// Returns tool-call ids from a successful Responses response. Binding these
/// ids lets the next request remain on the candidate that created the call,
/// even when a client changes the selected model between turns.
pub(super) fn response_tool_call_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    collect_response_tool_call_ids(value, &mut ids);
    ids
}

/// Drops incomplete Responses tool-call items from an imported conversation
/// history. This is only safe after an upstream has explicitly rejected the
/// missing result: Relay must never invent a tool output or remove a completed
/// tool turn during normal request processing.
pub(super) fn drop_unpaired_responses_tool_calls(request: &mut Value) -> bool {
    let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let output_ids = input
        .iter()
        .filter_map(response_tool_output_call_id)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let original_len = input.len();
    input.retain(|item| {
        response_tool_call_call_id(item)
            .is_none_or(|call_id| output_ids.iter().any(|output_id| output_id == call_id))
    });
    input.len() != original_len
}

fn response_tool_output_call_id(item: &Value) -> Option<&str> {
    let object = item.as_object()?;
    let kind = object.get("type")?.as_str()?;
    (kind == "tool_search_output" || kind.ends_with("_call_output"))
        .then(|| object.get("call_id"))
        .flatten()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|call_id| !call_id.is_empty() && call_id.len() <= 256)
}

fn response_tool_call_call_id(item: &Value) -> Option<&str> {
    let object = item.as_object()?;
    let kind = object.get("type")?.as_str()?;
    (kind == "function_call" || kind.ends_with("_call"))
        .then(|| object.get("call_id"))
        .flatten()
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|call_id| !call_id.is_empty() && call_id.len() <= 256)
}

fn collect_response_tool_call_ids(value: &Value, ids: &mut Vec<String>) {
    if ids.len() >= 16 {
        return;
    }
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_response_tool_call_ids(item, ids)),
        Value::Object(object) => {
            let kind = object.get("type").and_then(Value::as_str);
            if kind.is_some_and(|kind| kind == "function_call" || kind.ends_with("_call")) {
                for field in ["call_id", "id"] {
                    if let Some(call_id) = bounded_tool_call_id(object.get(field)) {
                        if !ids.iter().any(|known| known == &call_id) {
                            ids.push(call_id);
                        }
                    }
                }
            }
            object
                .values()
                .for_each(|item| collect_response_tool_call_ids(item, ids));
        }
        _ => {}
    }
}

fn bounded_tool_call_id(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && value.len() <= 256).then(|| value.to_string())
}

pub(super) fn tool_use_diagnostics(value: &Value) -> ToolUseDiagnostics {
    ToolUseDiagnostics {
        client_tool_count: tool_definition_count(value),
        tool_choice: tool_choice_mode(value),
        ..ToolUseDiagnostics::default()
    }
}

pub(super) fn with_forwarded_tool_diagnostics(
    client: &ToolUseDiagnostics,
    request_body: &[u8],
) -> ToolUseDiagnostics {
    let mut diagnostics = client.clone();
    diagnostics.forwarded_tool_count = serde_json::from_slice::<Value>(request_body)
        .ok()
        .map_or(0, |value| tool_definition_count(&value));
    diagnostics
}

fn tool_definition_count(value: &Value) -> u16 {
    let mut count = 0_u16;
    count = count.saturating_add(tool_array_count(value.get("tools")));
    count = count.saturating_add(tool_array_count(value.get("functions")));
    count = count.saturating_add(tool_array_count(
        value
            .get("response")
            .and_then(|response| response.get("tools")),
    ));
    if let Some(items) = value.get("input").and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                count = count.saturating_add(tool_array_count(item.get("tools")));
            }
        }
    }
    count
}

fn tool_array_count(value: Option<&Value>) -> u16 {
    value.and_then(Value::as_array).map_or(0, |tools| {
        tools.iter().fold(0_u16, |count, tool| {
            count.saturating_add(tool_definition_leaf_count(tool))
        })
    })
}

fn tool_definition_leaf_count(tool: &Value) -> u16 {
    if tool.get("type").and_then(Value::as_str) == Some("namespace") {
        let nested = tool_array_count(tool.get("tools"));
        return if nested == 0 { 1 } else { nested };
    }
    u16::from(tool.is_object())
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

pub(super) fn chat_request_uses_tools(value: &Value) -> bool {
    let Some(request) = value.as_object() else {
        return false;
    };
    ["tools", "functions", "tool_choice", "parallel_tool_calls"]
        .iter()
        .any(|field| request.contains_key(*field))
        || request
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| messages.iter().any(chat_message_uses_tools))
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

fn chat_message_uses_tools(message: &Value) -> bool {
    let Some(message) = message.as_object() else {
        return false;
    };
    matches!(
        message.get("role").and_then(Value::as_str),
        Some("tool" | "function")
    ) || ["tool_calls", "tool_call_id", "function_call"]
        .iter()
        .any(|field| message.contains_key(*field))
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
        DefaultServiceTier, GatewayRuntimeOptions, LocalGatewayKey, MessagesReasoningMode,
        ProviderSource, RuntimeLocalKey, RuntimeSource, SourceAdapter, SourceProtocolBinding,
    };
    use axum::http::{HeaderMap, HeaderValue};

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
        let forwarded =
            with_forwarded_tool_diagnostics(&diagnostics, &serde_json::to_vec(&request).unwrap());

        assert_eq!(diagnostics.client_tool_count, 5);
        assert_eq!(diagnostics.tool_choice, ToolChoiceMode::AllowedTools);
        assert_eq!(forwarded.forwarded_tool_count, 5);
        assert!(!serde_json::to_string(&forwarded)
            .unwrap()
            .contains("read_private_file"));
    }

    #[test]
    fn service_tier_defaults_inject_only_fast_without_overriding_client_choice() {
        let mut request = json!({});
        apply_default_service_tier_if_missing(&mut request, DefaultServiceTier::Fast);
        assert_eq!(request["service_tier"], "priority");
        assert_eq!(request_service_tier(&request), DefaultServiceTier::Fast);

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
        assert_eq!(
            response_tool_call_ids(&response),
            vec!["call_custom", "ctc_item"]
        );

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
    fn stale_responses_tool_calls_are_removed_only_when_their_output_is_missing() {
        let mut request = json!({
            "input": [
                {"type": "message", "role": "user", "content": "Continue"},
                {
                    "type": "custom_tool_call",
                    "id": "ctc_stale",
                    "call_id": "call_stale",
                    "name": "PowerShell",
                    "input": "Get-ChildItem"
                },
                {
                    "type": "function_call",
                    "id": "fc_completed",
                    "call_id": "call_completed",
                    "name": "pwd",
                    "arguments": "{}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_completed",
                    "output": "C:\\workspace"
                }
            ]
        });

        assert!(drop_unpaired_responses_tool_calls(&mut request));
        let input = request["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert!(input.iter().all(|item| item["call_id"] != "call_stale"));
        assert!(input.iter().any(|item| item["call_id"] == "call_completed"));

        assert!(!drop_unpaired_responses_tool_calls(&mut request));
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

        let response = build_codex_models_response(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            Some(&upstream),
        )
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
        let response =
            build_codex_models_response(&runtime, &key, &visible, &Default::default(), None)
                .unwrap();
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
    fn messages_bridge_adds_codex_ultra_only_as_a_translated_max_alias() {
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
                protocol_bindings: vec![SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::ResponsesToMessages,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: Default::default(),
                    model_ids: vec![model.into()],
                }],
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
        let response =
            build_codex_models_response(&runtime, &key, &visible, &Default::default(), None)
                .unwrap();

        assert_eq!(
            response["models"][0]["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|level| level["effort"].as_str())
                .collect::<Vec<_>>(),
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        // The model metadata remains the official five-level enum; `ultra`
        // exists only in the Codex projection that translates it to `max`.
        assert_eq!(
            runtime.model_capabilities(model).reasoning_effort_levels,
            ["low", "medium", "high", "xhigh", "max"]
        );
    }

    #[test]
    fn provider_reasoning_and_manual_overrides_do_not_grant_unknown_capabilities() {
        let runtime =
            capability_test_runtime(&["vendor/claude-fable-5"], GatewayRuntimeOptions::default());
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let source_reasoning = std::collections::BTreeMap::from([(
            "vendor/claude-fable-5".to_string(),
            json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Low"},
                    {"effort": "medium", "description": "Medium"},
                    {"effort": "high", "description": "High"},
                    {"effort": "ultra", "description": "Ultra"}
                ],
                "default_reasoning_level": "ultra",
                "supports_reasoning_summary_parameter": true,
                "supports_reasoning_summaries": true,
                "default_reasoning_summary": "detailed"
            })
            .as_object()
            .unwrap()
            .clone(),
        )]);

        let response = build_codex_models_response_with_source_reasoning(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            &source_reasoning,
            None,
        )
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
        let configured = build_codex_models_response_with_source_reasoning(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            &source_reasoning,
            None,
        )
        .expect("coding model catalog");
        let configured_model = &configured["models"][0];
        assert!(configured_model.get("default_reasoning_level").is_none());
        assert_eq!(configured_model["supported_reasoning_levels"], json!([]));

        runtime
            .set_model_reasoning_allowed_levels(std::collections::BTreeMap::new())
            .unwrap();
        let no_manual_selection = build_codex_models_response_with_source_reasoning(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            &source_reasoning,
            None,
        )
        .expect("coding model catalog");
        assert_eq!(
            no_manual_selection["models"][0]["supported_reasoning_levels"],
            json!([])
        );
    }

    #[test]
    fn api_source_image_capability_is_published_to_codex() {
        let runtime =
            capability_test_runtime(&["vendor/claude-fable-5"], GatewayRuntimeOptions::default());
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
        let image_models = std::collections::BTreeSet::from(["vendor/claude-fable-5".to_string()]);

        let response = build_codex_models_response_with_source_capabilities(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            &image_models,
            &Default::default(),
            None,
        )
        .expect("coding model catalog");

        assert_eq!(
            response["models"][0]["input_modalities"],
            json!(["text", "image"])
        );
        assert!(codex_catalog_entry_is_compatible(&response["models"][0]));
    }

    #[test]
    fn codex_catalog_uses_unique_priorities_and_keeps_unconfirmed_capabilities_disabled() {
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

        let response = build_codex_models_response(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            Some(&upstream),
        )
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
                "GPT 5.4",
            ]
        );
        assert!(models.iter().all(codex_catalog_entry_is_compatible));
        // A generic Responses source can reuse an OpenAI-looking model ID
        // without supporting Codex's native tool contract. Only account
        // manifests are authoritative for this capability.
        assert_eq!(models[0]["supports_parallel_tool_calls"], false);
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

        let response = build_codex_models_response(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            Some(&upstream),
        )
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
            ["GPT 5.6 Sol", "Claude Opus", "Grok"]
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
        let source_context_windows =
            std::collections::BTreeMap::from([("gpt-5.4".into(), 1_000_000)]);

        let response = build_codex_models_response(
            &runtime,
            &key,
            &visible,
            &source_context_windows,
            Some(&upstream),
        )
        .expect("coding model catalog");
        let model = &response["models"][0];

        assert!(model.get("context_window").is_none());
        assert!(model.get("max_context_window").is_none());
        assert!(model.get("auto_compact_token_limit").is_none());
    }
}

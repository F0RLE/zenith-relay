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
    forwarded_bridge_messages_headers, forwarded_codex_headers, forwarded_messages_headers,
};
pub(super) use normalization::{
    normalize_account_request, request_service_tier, try_recover_encrypted_content,
};

use super::execution::execute_client_request;
#[cfg(test)]
use crate::codex_catalog_entry_is_compatible;
use crate::{GatewayRuntime, ToolChoiceMode, ToolUseDiagnostics, WireApi};
use axum::body::Body;
use axum::extract::State;
#[cfg(test)]
use axum::http::HeaderValue;
use axum::http::{Request, Response};
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
        DefaultServiceTier, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource,
        RuntimeLocalKey, RuntimeSource,
    };

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
    fn service_tier_metrics_classify_legacy_fast_without_rewriting_the_request() {
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
    fn responses_lite_keeps_provider_owned_tools_and_choices_opaque() {
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
        assert_eq!(claude["input_modalities"], json!(["text", "image"]));
        assert!(models
            .iter()
            .any(|model| { model["slug"] == crate::codex_model_alias("disabled-code") }));
    }

    #[test]
    fn api_source_reasoning_metadata_is_visible_to_codex_without_native_model_identity() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec!["vendor/claude-fable-5".into()],
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
        assert_eq!(model["default_reasoning_level"], "medium");
        assert_eq!(
            model["supported_reasoning_levels"],
            json!([
                {"effort": "low", "description": "Low"},
                {"effort": "medium", "description": "Medium"},
                {"effort": "high", "description": "High"},
                {"effort": "ultra", "description": "Ultra"}
            ])
        );
        assert_eq!(model["supports_reasoning_summary_parameter"], true);
        assert_eq!(model["supports_reasoning_summaries"], true);
        assert_eq!(model["default_reasoning_summary"], "detailed");
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
        assert_eq!(configured_model["default_reasoning_level"], "ultra");
        assert_eq!(
            configured_model["supported_reasoning_levels"],
            json!([{"effort": "ultra", "description": "Ultra"}])
        );

        runtime
            .set_model_reasoning_allowed_levels(std::collections::BTreeMap::new())
            .unwrap();
        let automatic = build_codex_models_response_with_source_reasoning(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            &source_reasoning,
            None,
        )
        .expect("coding model catalog");
        assert_eq!(automatic["models"][0]["default_reasoning_level"], "medium");
    }

    #[test]
    fn api_source_image_capability_is_published_to_codex() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec!["vendor/claude-fable-5".into()],
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
                "GPT 5.4",
                "Claude Opus 4.8",
                "Gemini 3.6 Flash",
                "Grok 4.5",
                "GLM 5.2",
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
    fn source_context_replaces_stale_codex_context_for_matching_models() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-5.4".into()],
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

        assert_eq!(model["context_window"], 1_000_000);
        assert_eq!(model["max_context_window"], 1_000_000);
        assert!(model.get("auto_compact_token_limit").is_none());
    }
}

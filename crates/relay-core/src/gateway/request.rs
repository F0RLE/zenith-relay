use super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::errors::{api_error, AttemptFailure};
use super::execution::{execute_account_endpoint, execute_client_request};
use super::now_ms;
use crate::catalog::normalize_upstream_codex_catalog_entry;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::CODEX_MODELS_CLIENT_VERSION;
use crate::runtime::{AuthenticatedKey, DefaultServiceTier};
use crate::{
    codex_catalog_entry_is_compatible, codex_model_is_picker_eligible, routed_codex_catalog_entry,
    GatewayRuntime, WireApi,
};
use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) const MAX_CLIENT_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

pub(super) const MAX_CLIENT_REQUEST_BODY_ERROR: &str = "request body exceeds 64 MiB";

const MAX_CODEX_MODELS_BODY_BYTES: usize = 512 * 1024;

const MAX_ALPHA_SEARCH_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

pub(super) const CODEX_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";

const CLAUDE_CODE_SESSION_HEADER: &str = "x-claude-code-session-id";

const FORWARDED_CODEX_HEADERS: &[&str] = &[
    "openai-beta",
    "originator",
    "session-id",
    "session_id",
    "thread-id",
    "traceparent",
    "tracestate",
    "user-agent",
    "version",
    "x-claude-code-session-id",
    "x-client-request-id",
    "x-codex-beta-features",
    "x-codex-installation-id",
    "x-codex-parent-thread-id",
    "x-codex-turn-metadata",
    "x-codex-turn-state",
    "x-codex-window-id",
    "x-oai-attestation",
    "x-openai-memgen-request",
    "x-openai-subagent",
    "x-responsesapi-include-timing-metrics",
    "x-session-id",
];

pub(super) fn forwarded_codex_headers(
    client_headers: &HeaderMap,
    fallback_session_id: &str,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for &name in FORWARDED_CODEX_HEADERS {
        if let Some(value) = client_headers.get(name) {
            headers.insert(HeaderName::from_static(name), value.clone());
        }
    }
    if !headers.contains_key(CLAUDE_CODE_SESSION_HEADER) {
        let session_id = ["session_id", "x-session-id", "session-id", "thread-id"]
            .iter()
            .find_map(|name| client_headers.get(*name))
            .cloned()
            .or_else(|| HeaderValue::from_str(fallback_session_id).ok());
        if let Some(session_id) = session_id {
            headers.insert(
                HeaderName::from_static(CLAUDE_CODE_SESSION_HEADER),
                session_id,
            );
        }
    }
    headers
}

pub(super) async fn models(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    let protocols = allowed_model_protocols(&runtime, &key);
    let models = runtime.visible_models(&key, &protocols, now_ms());
    let client_version = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "client_version")
            .map(|(_, value)| value.into_owned())
    });
    if let Some(client_version) = client_version.as_deref() {
        if !valid_codex_client_version(client_version) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "client_version is invalid",
                "invalid_request",
            );
        }
        if let Some(catalog) =
            codex_models_response(runtime.as_ref(), &key, &protocols, &models, client_version).await
        {
            return Json(catalog).into_response();
        }
    }
    Json(json!({
        "object": "list",
        "data": models.into_iter().map(|id| json!({
            "id": id,
            "object": "model",
            "owned_by": "zenith-relay",
        })).collect::<Vec<_>>()
    }))
    .into_response()
}

async fn codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    allowed_protocols: &[WireApi],
    visible_models: &[String],
    client_version: &str,
) -> Option<Value> {
    let now_ms = now_ms();
    let source_context_windows = runtime
        .codex_source_context_windows(key, allowed_protocols, now_ms)
        .await;
    let routes = runtime.codex_models_routes(key, now_ms).await;
    let candidate_ids = routes
        .iter()
        .map(|(candidate_id, _)| candidate_id.clone())
        .collect::<Vec<_>>();
    let client_versions = if client_version == CODEX_MODELS_CLIENT_VERSION {
        vec![client_version]
    } else {
        vec![client_version, CODEX_MODELS_CLIENT_VERSION]
    };
    for (candidate_id, mut url) in routes {
        for client_version in &client_versions {
            url.query_pairs_mut()
                .clear()
                .append_pair("client_version", client_version);
            let request = runtime
                .request_client(&candidate_id, false)
                .get(url.clone())
                .timeout(Duration::from_secs(10));
            let Ok(response) = runtime
                .send_authorized_request(&candidate_id, request, Some(client_version))
                .await
            else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            let Ok(body) =
                crate::runtime::collect_limited(response, MAX_CODEX_MODELS_BODY_BYTES).await
            else {
                continue;
            };
            if let Some(catalog) = codex_models_from_upstream(
                runtime,
                key,
                visible_models,
                &source_context_windows,
                &body,
            ) {
                runtime.clear_candidate_capability_blocks(&candidate_id);
                if let Ok(upstream) = serde_json::from_slice::<Value>(&body) {
                    runtime.remember_codex_model_manifest(&candidate_id, upstream, now_ms);
                }
                return Some(catalog);
            }
        }
    }
    let stale = runtime.stale_codex_model_manifest(candidate_ids.iter().map(String::as_str));
    build_codex_models_response(
        runtime,
        key,
        visible_models,
        &source_context_windows,
        stale.as_ref(),
    )
}

fn codex_models_from_upstream(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    source_context_windows: &std::collections::BTreeMap<String, u64>,
    body: &[u8],
) -> Option<Value> {
    let payload: Value = serde_json::from_slice(body).ok()?;
    build_codex_models_response(
        runtime,
        key,
        visible_models,
        source_context_windows,
        Some(&payload),
    )
}

fn build_codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    source_context_windows: &std::collections::BTreeMap<String, u64>,
    upstream: Option<&Value>,
) -> Option<Value> {
    let upstream_models = upstream
        .and_then(|payload| payload.get("models"))
        .and_then(Value::as_array)
        .filter(|models| models.len() <= 4_096);
    let visible = visible_models
        .iter()
        .filter_map(|display_id| {
            runtime.resolve_model(key, display_id).map(|upstream_id| {
                (
                    upstream_id.to_ascii_lowercase(),
                    (upstream_id, display_id.clone()),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut models = upstream_models
        .into_iter()
        .flat_map(|models| models.iter())
        .filter_map(|model| {
            let model = model.as_object()?.clone();
            let slug = model.get("slug")?.as_str()?.trim();
            if slug.is_empty() || slug.len() > 256 || slug.chars().any(char::is_control) {
                return None;
            }
            let normalized = slug.to_ascii_lowercase();
            if !codex_model_is_picker_eligible(slug)
                || model.get("supported_in_api") == Some(&Value::Bool(false))
                || model
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("hide"))
            {
                return None;
            }
            let (upstream_id, display_id) = visible.get(&normalized)?;
            if seen.contains(&normalized) {
                return None;
            }
            let (upstream_id, display_id) = (upstream_id.clone(), display_id.clone());
            let mut model = normalize_upstream_codex_catalog_entry(
                &model,
                &display_id,
                1_000 + seen.len() as u64,
                source_context_windows
                    .get(&upstream_id.to_ascii_lowercase())
                    .copied(),
            )?;
            seen.insert(normalized);
            if model
                .get("use_responses_lite")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                runtime.remember_codex_responses_lite_model(slug);
                model["supports_parallel_tool_calls"] = Value::Bool(false);
            }
            if let Some(context_window) =
                source_context_windows.get(&upstream_id.to_ascii_lowercase())
            {
                model["context_window"] = (*context_window).into();
                model["max_context_window"] = (*context_window).into();
                model
                    .as_object_mut()
                    .expect("normalized catalog entry is an object")
                    .remove("auto_compact_token_limit");
                model["effective_context_window_percent"] = 95.into();
            }
            Some(model)
        })
        .collect::<Vec<_>>();

    let template = upstream_models.and_then(|models| {
        models
            .iter()
            .find(|model| codex_catalog_entry_is_compatible(model))
            .and_then(Value::as_object)
    });
    let mut routed = visible
        .into_iter()
        .filter(|(normalized, (model, _))| {
            !seen.contains(normalized) && codex_model_is_picker_eligible(model)
        })
        .collect::<Vec<_>>();
    routed.sort_by(|left, right| left.0.cmp(&right.0));
    for (index, (_, (upstream_id, display_id))) in routed.into_iter().enumerate() {
        models.push(routed_codex_catalog_entry(
            template,
            &display_id,
            1_000 + index as u64,
            source_context_windows
                .get(&upstream_id.to_ascii_lowercase())
                .copied(),
        ));
    }

    if models.is_empty() {
        None
    } else {
        Some(json!({ "models": models }))
    }
}

fn valid_codex_client_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_'))
}

pub(super) fn normalize_service_tier(
    object: &mut Map<String, Value>,
    default_service_tier: DefaultServiceTier,
) {
    if default_service_tier == DefaultServiceTier::Fast {
        object.insert(
            "service_tier".to_string(),
            Value::String("priority".to_string()),
        );
    } else if let Some(Value::String(value)) = object.get_mut("service_tier") {
        match value.to_ascii_lowercase().as_str() {
            "fast" => *value = "priority".to_string(),
            "standard" => *value = "default".to_string(),
            _ => {}
        }
    }
}

pub(super) fn request_service_tier(request: &Value) -> DefaultServiceTier {
    if request
        .get("service_tier")
        .and_then(Value::as_str)
        .is_some_and(|tier| tier.eq_ignore_ascii_case("priority"))
    {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

pub(super) async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::Responses).await
}

pub(super) async fn responses_compact(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::Responses) {
        return client_api_forbidden();
    }
    let mut request = match read_json_object(body).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.get("stream").is_some_and(|stream| stream != false) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "streaming is not supported for compact responses",
            "invalid_request",
        );
    }
    normalize_service_tier(&mut request, runtime.default_service_tier());
    let Some(requested_model) = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a non-empty string",
            "invalid_request",
        );
    };
    let Some(resolved_model) = resolve_visible_account_model(&runtime, &key, &requested_model)
    else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    };
    let responses_lite = headers
        .get(CODEX_RESPONSES_LITE_HEADER)
        .cloned()
        .or_else(|| {
            runtime
                .codex_model_uses_responses_lite(&resolved_model)
                .then(|| HeaderValue::from_static("true"))
        });
    let response_affinity_key =
        runtime.response_affinity_key(request.get("previous_response_id").and_then(Value::as_str));
    normalize_account_request(&mut request, responses_lite.is_some());
    request.remove("stream");
    execute_account_endpoint(
        runtime,
        key,
        Value::Object(request),
        requested_model,
        resolved_model,
        headers,
        AccountEndpoint::Compact,
        responses_lite,
        response_affinity_key,
        true,
    )
    .await
}

pub(super) async fn alpha_search(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::Responses) {
        return client_api_forbidden();
    }
    let mut request = match read_json_object(body).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let model_was_provided = request
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| !model.trim().is_empty());
    let requested_model = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
        .or_else(|| runtime.visible_account_models(&key).into_iter().next());
    let Some(requested_model) = requested_model else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no OAuth account model is available for search",
            "no_eligible_source",
        );
    };
    let Some(resolved_model) = resolve_visible_account_model(&runtime, &key, &requested_model)
    else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    };
    if !model_was_provided {
        request.remove("model");
    }
    request.remove("prompt_cache_key");
    request.remove("prompt_cache_retention");
    if let Some(session_id) = request
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .and_then(|value| HeaderValue::from_str(value).ok())
    {
        if !headers.contains_key("x-session-id") {
            headers.insert("x-session-id", session_id.clone());
        }
        if !headers.contains_key("session_id") {
            headers.insert("session_id", session_id);
        }
    }
    execute_account_endpoint(
        runtime,
        key,
        Value::Object(request),
        requested_model,
        resolved_model,
        headers,
        AccountEndpoint::AlphaSearch,
        None,
        None,
        model_was_provided,
    )
    .await
}

pub(super) async fn chat_completions(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::ChatCompletions).await
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum AccountEndpoint {
    Compact,
    AlphaSearch,
}

impl AccountEndpoint {
    pub(super) fn response_limit(self) -> usize {
        match self {
            Self::Compact => crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            Self::AlphaSearch => MAX_ALPHA_SEARCH_RESPONSE_BYTES,
        }
    }
}

async fn read_json_object(body: Body) -> Result<Map<String, Value>, Response<Body>> {
    let body = axum::body::to_bytes(body, MAX_CLIENT_REQUEST_BODY_BYTES)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                MAX_CLIENT_REQUEST_BODY_ERROR,
                "request_too_large",
            )
        })?;
    match serde_json::from_slice(&body) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
            "invalid_request",
        )),
    }
}

fn resolve_visible_account_model(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    requested_model: &str,
) -> Option<String> {
    runtime.resolve_visible_account_model(key, requested_model)
}

pub(super) fn account_endpoint_url(
    mut responses_url: url::Url,
    endpoint: AccountEndpoint,
) -> Option<url::Url> {
    let mut segments = responses_url.path_segments_mut().ok()?;
    segments.pop_if_empty().pop();
    match endpoint {
        AccountEndpoint::Compact => {
            segments.push("responses").push("compact");
        }
        AccountEndpoint::AlphaSearch => {
            segments.push("alpha").push("search");
        }
    }
    drop(segments);
    Some(responses_url)
}

pub(super) fn normalize_account_request_body(
    body: &[u8],
    responses_lite: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let mut request =
        serde_json::from_slice::<Value>(body).map_err(|_| AttemptFailure::invalid_request())?;
    let object = request
        .as_object_mut()
        .ok_or_else(AttemptFailure::invalid_request)?;
    normalize_account_request(object, responses_lite);
    serde_json::to_vec(&request).map_err(|_| AttemptFailure::invalid_request())
}

pub(super) fn normalize_account_request(
    object: &mut serde_json::Map<String, Value>,
    responses_lite: bool,
) {
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    object.remove("max_output_tokens");
    sanitize_unstored_reasoning_items(object);
    if responses_lite {
        // Codex Responses Lite accepts only complete reasoning history.  Keep
        // the client-selected effort and summary settings, but always supply
        // the mandatory context mode before the request reaches either the
        // HTTP or WebSocket account transport.
        let reasoning = object
            .entry("reasoning".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !reasoning.is_object() {
            *reasoning = Value::Object(Map::new());
        }
        reasoning
            .as_object_mut()
            .expect("reasoning was normalized to an object")
            .insert(
                "context".to_string(),
                Value::String("all_turns".to_string()),
            );
        object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
        filter_responses_lite_tools(object);
    }
    match object.get("input") {
        Some(Value::String(text)) if text.trim().is_empty() => {
            object.insert("input".to_string(), Value::Array(Vec::new()));
        }
        Some(Value::String(text)) => {
            object.insert(
                "input".to_string(),
                json!([{"role": "user", "content": [{"type": "input_text", "text": text}]}]),
            );
        }
        Some(Value::Object(item)) => {
            object.insert(
                "input".to_string(),
                Value::Array(vec![Value::Object(item.clone())]),
            );
        }
        _ => {}
    }
}

fn sanitize_unstored_reasoning_items(object: &mut Map<String, Value>) {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            continue;
        }
        let has_encrypted_content = item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if !has_encrypted_content {
            item.remove("id");
            item.remove("encrypted_content");
        }
    }
}

pub(super) fn try_recover_encrypted_content(request: &mut Value, attempted: &mut bool) -> bool {
    if *attempted {
        return false;
    }
    let mut recovered = request.clone();
    let mut changed = false;
    strip_encrypted_reasoning(&mut recovered, &mut changed);
    if !changed {
        return false;
    }
    *request = recovered;
    *attempted = true;
    true
}

fn strip_encrypted_reasoning(value: &mut Value, changed: &mut bool) {
    match value {
        Value::Array(values) => {
            for value in values {
                strip_encrypted_reasoning(value, changed);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("reasoning")
                && object
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.trim().is_empty())
            {
                object.remove("encrypted_content");
                object.remove("id");
                *changed = true;
            }
            for value in object.values_mut() {
                strip_encrypted_reasoning(value, changed);
            }
        }
        _ => {}
    }
}

fn filter_responses_lite_tools(object: &mut Map<String, Value>) {
    if let Some(Value::Array(tools)) = object.get_mut("tools") {
        tools.retain(responses_lite_tool_allowed);
    }
    if object
        .get_mut("tool_choice")
        .is_some_and(|choice| !responses_lite_tool_choice_allowed(choice))
    {
        object.remove("tool_choice");
    }
    if let Some(Value::Array(input)) = object.get_mut("input") {
        input.retain_mut(|item| {
            let Some(item) = item.as_object_mut() else {
                return true;
            };
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                return true;
            }
            filter_responses_lite_tools(item);
            item.get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| !tools.is_empty())
        });
    }
    if let Some(Value::Object(response)) = object.get_mut("response") {
        filter_responses_lite_tools(response);
    }
}

fn responses_lite_tool_allowed(tool: &Value) -> bool {
    let Some(tool_type) = tool.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    if ["function", "custom", "namespace"]
        .iter()
        .any(|allowed| tool_type.eq_ignore_ascii_case(allowed))
    {
        return true;
    }
    tool_type.eq_ignore_ascii_case("tool_search")
        && tool
            .get("execution")
            .and_then(Value::as_str)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("client"))
}

fn responses_lite_tool_choice_allowed(choice: &mut Value) -> bool {
    if let Some(choice) = choice.as_str() {
        return ["auto", "none", "required"]
            .iter()
            .any(|value| choice.trim().eq_ignore_ascii_case(value));
    }
    let Some(choice) = choice.as_object_mut() else {
        return false;
    };
    let Some(choice_type) = choice.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    if ["function", "custom", "namespace"]
        .iter()
        .any(|allowed| choice_type.eq_ignore_ascii_case(allowed))
    {
        return true;
    }
    if choice_type.eq_ignore_ascii_case("tool_search") {
        return choice
            .get("execution")
            .and_then(Value::as_str)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("client"));
    }
    if choice_type.eq_ignore_ascii_case("allowed_tools") {
        let mut any_allowed = false;
        for name in ["tools", "allowed_tools"] {
            if let Some(tools) = choice.get_mut(name).and_then(Value::as_array_mut) {
                tools.retain(responses_lite_tool_allowed);
                any_allowed |= !tools.is_empty();
            }
        }
        return any_allowed;
    }
    false
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

pub(super) fn candidate_protocols(wire_api: WireApi) -> &'static [WireApi] {
    match wire_api {
        WireApi::Responses => &[WireApi::Responses, WireApi::ChatCompletions],
        WireApi::ChatCompletions => &[WireApi::ChatCompletions, WireApi::Responses],
        WireApi::Messages => &[WireApi::Messages],
    }
}

fn allowed_model_protocols(runtime: &GatewayRuntime, key: &AuthenticatedKey) -> Vec<WireApi> {
    let mut protocols = Vec::new();
    if runtime.allows_client_wire_api(key, ClientWireApi::Responses) {
        protocols.push(WireApi::Responses);
    }
    if runtime.allows_client_wire_api(key, ClientWireApi::ChatCompletions) {
        protocols.push(WireApi::ChatCompletions);
    }
    protocols
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
        GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource,
    };

    #[test]
    fn forwarded_codex_headers_keep_session_identity_and_drop_secrets() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("x-session-id", HeaderValue::from_static("session-42"));
        client_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer local-secret"),
        );
        client_headers.insert("cookie", HeaderValue::from_static("session=secret"));
        client_headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("private-account"),
        );

        let forwarded = forwarded_codex_headers(&client_headers, "relay-request");
        assert_eq!(forwarded["x-session-id"], "session-42");
        assert_eq!(forwarded[CLAUDE_CODE_SESSION_HEADER], "session-42");
        assert!(!forwarded.contains_key(AUTHORIZATION));
        assert!(!forwarded.contains_key("cookie"));
        assert!(!forwarded.contains_key("chatgpt-account-id"));

        let synthesized = forwarded_codex_headers(&HeaderMap::new(), "relay-request");
        assert_eq!(synthesized[CLAUDE_CODE_SESSION_HEADER], "relay-request");
    }

    #[test]
    fn standard_mode_follows_each_chat_service_tier() {
        let mut standard = json!({"service_tier": "standard"});
        normalize_service_tier(
            standard.as_object_mut().unwrap(),
            DefaultServiceTier::Standard,
        );
        assert_eq!(standard["service_tier"], "default");

        let mut fast = json!({"service_tier": "fast"});
        normalize_service_tier(fast.as_object_mut().unwrap(), DefaultServiceTier::Standard);
        assert_eq!(fast["service_tier"], "priority");

        let mut inherited = json!({});
        normalize_service_tier(
            inherited.as_object_mut().unwrap(),
            DefaultServiceTier::Standard,
        );
        assert!(inherited.get("service_tier").is_none());
    }

    #[test]
    fn responses_lite_keeps_only_client_executed_tools() {
        let mut request = json!({
            "model": "gpt-lite",
            "tools": [
                {"type": "function", "name": "lookup"},
                {"type": "custom", "name": "patch"},
                {"type": "namespace", "name": "collaboration", "tools": [
                    {"type": "function", "name": "spawn_agent"},
                    {"type": "function", "name": "wait_agent"}
                ]},
                {"type": "tool_search", "execution": "client"},
                {"type": "tool_search", "execution": "server"},
                {"type": "web_search"},
                {"type": "image_generation"}
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "tools": [
                    {"type": "function", "name": "lookup"},
                    {"type": "namespace", "name": "collaboration"},
                    {"type": "web_search"}
                ]
            },
            "input": [
                {"type": "additional_tools", "tools": [{"type": "web_search"}]},
                {"type": "additional_tools", "tools": [
                    {"type": "custom", "name": "patch"},
                    {"type": "image_generation"}
                ]},
                {"role": "user", "content": "hello"}
            ],
            "response": {
                "tools": [{"type": "function", "name": "lookup"}, {"type": "web_search"}],
                "tool_choice": {"type": "image_generation"}
            }
        });
        normalize_account_request(request.as_object_mut().unwrap(), true);

        let types = |pointer: &str| {
            request
                .pointer(pointer)
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .filter_map(|tool| tool.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            types("/tools"),
            ["function", "custom", "namespace", "tool_search"]
        );
        assert_eq!(types("/tool_choice/tools"), ["function", "namespace"]);
        assert_eq!(types("/input/0/tools"), ["custom"]);
        assert_eq!(types("/response/tools"), ["function"]);
        assert_eq!(request["input"].as_array().unwrap().len(), 2);
        assert!(request.pointer("/response/tool_choice").is_none());
        assert_eq!(request["parallel_tool_calls"], false);
    }

    #[test]
    fn responses_lite_preserves_namespace_tool_choice() {
        let mut request = json!({
            "tools": [{"type": "namespace", "name": "collaboration"}],
            "tool_choice": {"type": "namespace", "name": "collaboration"}
        });

        normalize_account_request(request.as_object_mut().unwrap(), true);

        assert_eq!(request["tools"][0]["type"], "namespace");
        assert_eq!(request["tool_choice"]["type"], "namespace");
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
        assert!(models
            .iter()
            .any(|model| { model["slug"] == crate::codex_model_alias("disabled-code") }));
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

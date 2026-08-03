use super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::errors::api_error;
use super::execution::{execute_account_endpoint, execute_client_request};
use super::now_ms;
use crate::catalog::{
    canonicalize_model_ids, normalize_codex_catalog_priorities,
    normalize_upstream_codex_catalog_entry,
};
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::CODEX_MODELS_CLIENT_VERSION;
use crate::runtime::{AuthenticatedKey, DefaultServiceTier};
use crate::{
    codex_catalog_entry_is_compatible, codex_model_is_picker_eligible, routed_codex_catalog_entry,
    GatewayRuntime, ToolChoiceMode, ToolUseDiagnostics, WireApi,
};
use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
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

/// Credentials supplied by a Relay client authenticate only the local
/// gateway. They must never be forwarded to a configured upstream source,
/// which authenticates with its own stored credential.
fn is_client_auth_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-auth-token"
            | "x-api-token"
    ) || name.ends_with("-api-key")
}

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

/// A Responses-to-Messages bridge receives a Codex/Responses client request,
/// not a native Anthropic client request. Carry only the metadata that has a
/// defined Messages-side meaning; forwarding OpenAI/Codex headers would leak
/// private client state into an unrelated upstream contract.
pub(super) fn forwarded_bridge_messages_headers(client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in ["user-agent", CLAUDE_CODE_SESSION_HEADER] {
        if let Some(value) = client_headers.get(name) {
            headers.insert(HeaderName::from_static(name), value.clone());
        }
    }
    headers
}

/// For native Messages routes, forward only headers that belong to the
/// Anthropic contract. This avoids leaking Codex/OpenAI request metadata into
/// a different upstream protocol while retaining the version and session
/// details needed by Claude Code.
pub(super) fn forwarded_messages_headers(client_headers: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in client_headers {
        let name = name.as_str();
        let is_messages_metadata = name == "user-agent"
            || name.starts_with("anthropic-")
            || name.starts_with("x-claude-")
            || name.starts_with("x-stainless-");
        if is_messages_metadata && !is_client_auth_header(name) {
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).expect("request header name is valid"),
                value.clone(),
            );
        }
    }
    headers
        .entry(HeaderName::from_static("anthropic-version"))
        .or_insert_with(|| HeaderValue::from_static("2023-06-01"));
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
    let client_version = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "client_version")
            .map(|(_, value)| value.into_owned())
    });
    let protocols = match client_version.as_deref() {
        // Codex always executes selected models through /v1/responses.  Do
        // not publish a model there merely because it is available through a
        // different native endpoint: doing so creates a picker entry that can
        // never complete its first request.
        Some(_) => allowed_codex_model_protocols(&runtime, &key),
        // The generic OpenAI-compatible list is shared by Responses and Chat
        // Completions clients.  Anthropic Messages has its own native
        // discovery contract and must not be presented as an OpenAI model.
        None => allowed_openai_model_protocols(&runtime, &key),
    };
    let models = runtime.visible_models(&key, &protocols, now_ms());
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
    let mut visible = visible_models
        .iter()
        .filter_map(|display_id| {
            runtime.resolve_model(key, display_id).map(|upstream_id| {
                (
                    upstream_id.to_ascii_lowercase(),
                    (upstream_id, display_id.clone()),
                )
            })
        })
        .collect::<Vec<_>>();
    if visible.is_empty() {
        return None;
    }
    let picker_positions = canonicalize_model_ids(
        visible
            .iter()
            .map(|(_, (_, display_id))| display_id.as_str()),
    )
    .into_iter()
    .enumerate()
    .map(|(position, id)| (id.to_ascii_lowercase(), position))
    .collect::<HashMap<_, _>>();
    visible.sort_by_key(|(_, (_, display_id))| {
        picker_positions
            .get(&display_id.to_ascii_lowercase())
            .copied()
            .unwrap_or(usize::MAX)
    });

    // Source models remain provider-agnostic in the runtime. The picker is the
    // presentation boundary: it groups familiar model IDs while the upstream
    // catalog only supplies capability templates for those same IDs.
    let upstream_by_model = upstream_models
        .into_iter()
        .flat_map(|models| models.iter())
        .filter_map(|model| {
            let object = model.as_object()?;
            let slug = object.get("slug")?.as_str()?.trim();
            if slug.is_empty() || slug.len() > 256 || slug.chars().any(char::is_control) {
                return None;
            }
            let normalized = slug.to_ascii_lowercase();
            if !codex_model_is_picker_eligible(slug)
                || object.get("supported_in_api") == Some(&Value::Bool(false))
                || object
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("hide"))
            {
                return None;
            }
            visible
                .iter()
                .any(|(upstream_id, _)| upstream_id == &normalized)
                .then_some((normalized, object.clone()))
        })
        .fold(HashMap::new(), |mut entries, (normalized, object)| {
            entries.entry(normalized).or_insert(object);
            entries
        });

    let template = upstream_models.and_then(|models| {
        models
            .iter()
            .find(|model| codex_catalog_entry_is_compatible(model))
            .and_then(Value::as_object)
    });
    let mut models = Vec::with_capacity(visible.len());
    for (index, (normalized, (upstream_id, display_id))) in visible.into_iter().enumerate() {
        if !codex_model_is_picker_eligible(&upstream_id) {
            continue;
        }
        let priority = crate::CODEX_CATALOG_PRIORITY_BASE.saturating_add(index as u64);
        let Some(mut model) = upstream_by_model
            .get(&normalized)
            .and_then(|entry| {
                normalize_upstream_codex_catalog_entry(
                    entry,
                    &display_id,
                    priority,
                    source_context_windows
                        .get(&upstream_id.to_ascii_lowercase())
                        .copied(),
                )
            })
            .or_else(|| {
                Some(routed_codex_catalog_entry(
                    template,
                    &display_id,
                    priority,
                    source_context_windows
                        .get(&upstream_id.to_ascii_lowercase())
                        .copied(),
                ))
            })
        else {
            continue;
        };
        if upstream_by_model
            .get(&normalized)
            .and_then(|entry| entry.get("use_responses_lite"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            runtime.remember_codex_responses_lite_model(&upstream_id);
        }
        if let Some(context_window) = source_context_windows.get(&upstream_id.to_ascii_lowercase())
        {
            model["context_window"] = (*context_window).into();
            model["max_context_window"] = (*context_window).into();
            model
                .as_object_mut()
                .expect("normalized catalog entry is an object")
                .remove("auto_compact_token_limit");
            model["effective_context_window_percent"] = 95.into();
        }
        models.push(model);
    }

    normalize_codex_catalog_priorities(&mut models);
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

pub(super) async fn messages(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    super::messages::native_messages_error_response(
        execute_client_request(runtime, request, WireApi::Messages).await,
    )
    .await
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
        tools.retain_mut(sanitize_responses_lite_tool);
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
    let available_tools = responses_lite_available_tools(object);
    if object
        .get_mut("tool_choice")
        .is_some_and(|choice| !responses_lite_tool_choice_allowed(choice, &available_tools))
    {
        object.remove("tool_choice");
    }
}

fn sanitize_responses_lite_tool(tool: &mut Value) -> bool {
    if !responses_lite_tool_allowed(tool) {
        return false;
    }
    let Some(object) = tool.as_object_mut() else {
        return false;
    };
    if !object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|tool_type| tool_type.eq_ignore_ascii_case("namespace"))
    {
        return true;
    }
    let Some(children) = object.get_mut("tools").and_then(Value::as_array_mut) else {
        return true;
    };
    children.retain_mut(sanitize_responses_lite_namespace_child);
    !children.is_empty()
}

fn sanitize_responses_lite_namespace_child(tool: &mut Value) -> bool {
    if tool.get("type").is_none() {
        return !responses_lite_tool_is_server_executed(tool)
            && tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(valid_client_tool_name);
    }
    sanitize_responses_lite_tool(tool)
}

fn responses_lite_available_tools(object: &Map<String, Value>) -> Vec<Value> {
    let mut tools = object
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                continue;
            }
            if let Some(additional) = item.get("tools").and_then(Value::as_array) {
                tools.extend(additional.iter().cloned());
            }
        }
    }
    tools
}

fn responses_lite_tool_allowed(tool: &Value) -> bool {
    let Some(tool_type) = tool
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    if responses_lite_server_tool(tool_type) || responses_lite_tool_is_server_executed(tool) {
        return false;
    }
    if ["function", "custom", "namespace"]
        .iter()
        .any(|allowed| tool_type.eq_ignore_ascii_case(allowed))
    {
        return true;
    }
    if tool_type.eq_ignore_ascii_case("tool_search") {
        // Codex has sent the deferred discovery tool both with and without an
        // explicit execution marker.  It is always client-side unless the
        // marker explicitly says server, so do not erase the only route to
        // deferred tools merely because an optional field is absent.
        return true;
    }
    tool.get("name")
        .and_then(Value::as_str)
        .is_some_and(valid_client_tool_name)
}

fn responses_lite_tool_is_server_executed(tool: &Value) -> bool {
    tool.get("execution")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("server"))
}

fn responses_lite_server_tool(tool_type: &str) -> bool {
    let tool_type = tool_type.trim().to_ascii_lowercase();
    [
        "web_search",
        "web_search_preview",
        "image_generation",
        "image_gen",
        "file_search",
        "computer_use_preview",
        "code_interpreter",
        "mcp",
    ]
    .iter()
    .any(|server_tool| tool_type == *server_tool)
        || [
            "web_search_",
            "image_generation_",
            "file_search_",
            "computer_use_",
            "code_interpreter_",
            "mcp_",
        ]
        .iter()
        .any(|prefix| tool_type.starts_with(prefix))
}

fn valid_client_tool_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.trim() == name
        && name.len() <= 256
        && !name.chars().any(char::is_control)
}

fn responses_lite_tool_choice_allowed(choice: &mut Value, available_tools: &[Value]) -> bool {
    if let Some(choice) = choice.as_str() {
        let choice = choice.trim();
        return if choice.eq_ignore_ascii_case("auto") || choice.eq_ignore_ascii_case("none") {
            true
        } else if choice.eq_ignore_ascii_case("required") {
            !available_tools.is_empty()
        } else {
            false
        };
    }
    let Some(choice) = choice.as_object_mut() else {
        return false;
    };
    let Some(choice_type) = choice.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    if !choice_type.eq_ignore_ascii_case("allowed_tools") {
        return responses_lite_tool_choice_matches_available(choice, available_tools);
    }
    let mut any_allowed = false;
    for name in ["tools", "allowed_tools"] {
        if let Some(tools) = choice.get_mut(name).and_then(Value::as_array_mut) {
            tools.retain(|tool| {
                tool.as_object().is_some_and(|tool| {
                    responses_lite_tool_choice_matches_available(tool, available_tools)
                })
            });
            any_allowed |= !tools.is_empty();
        }
    }
    any_allowed
}

fn responses_lite_tool_choice_matches_available(
    choice: &serde_json::Map<String, Value>,
    available_tools: &[Value],
) -> bool {
    let Some(choice_type) = choice.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    if responses_lite_server_tool(choice_type)
        || choice
            .get("execution")
            .and_then(Value::as_str)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("server"))
    {
        return false;
    }
    available_tools
        .iter()
        .any(|tool| responses_lite_tool_choice_matches_definition(choice, tool, None))
}

fn responses_lite_tool_choice_matches_definition(
    choice: &serde_json::Map<String, Value>,
    definition: &Value,
    namespace: Option<&str>,
) -> bool {
    let Some(definition) = definition.as_object() else {
        return false;
    };
    let Some(definition_type) = definition
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
    else {
        return false;
    };
    if definition_type.eq_ignore_ascii_case("namespace") {
        let Some(definition_namespace) = definition
            .get("name")
            .or_else(|| definition.get("namespace"))
            .and_then(Value::as_str)
            .filter(|value| valid_client_tool_name(value))
        else {
            return false;
        };
        if choice
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|choice_type| choice_type.eq_ignore_ascii_case("namespace"))
            && choice
                .get("name")
                .or_else(|| choice.get("namespace"))
                .and_then(Value::as_str)
                == Some(definition_namespace)
        {
            return true;
        }
        return definition
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children.iter().any(|child| {
                    responses_lite_tool_choice_matches_namespace_child(
                        choice,
                        child,
                        definition_namespace,
                    )
                })
            });
    }
    responses_lite_tool_choice_matches_leaf(choice, definition, namespace)
}

fn responses_lite_tool_choice_matches_namespace_child(
    choice: &serde_json::Map<String, Value>,
    definition: &Value,
    namespace: &str,
) -> bool {
    let Some(object) = definition.as_object() else {
        return false;
    };
    if object.get("type").is_none() {
        return responses_lite_tool_choice_matches_leaf(choice, object, Some(namespace));
    }
    responses_lite_tool_choice_matches_definition(choice, definition, Some(namespace))
}

fn responses_lite_tool_choice_matches_leaf(
    choice: &serde_json::Map<String, Value>,
    definition: &serde_json::Map<String, Value>,
    namespace: Option<&str>,
) -> bool {
    let Some(choice_type) = choice.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    let definition_type = definition
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("function");
    if !choice_type.eq_ignore_ascii_case(definition_type) {
        return false;
    }
    if namespace.is_some() && choice.get("namespace").and_then(Value::as_str) != namespace {
        return false;
    }
    let definition_name = definition
        .get("name")
        .or_else(|| {
            definition
                .get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(Value::as_str);
    let choice_name = choice
        .get("name")
        .or_else(|| {
            choice
                .get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(Value::as_str);
    match (definition_name, choice_name) {
        (None, None) => choice_type.eq_ignore_ascii_case("tool_search"),
        (Some(definition_name), Some(choice_name)) => definition_name == choice_name,
        _ => false,
    }
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

fn allowed_openai_model_protocols(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
) -> Vec<WireApi> {
    let mut protocols = Vec::new();
    if runtime.allows_client_wire_api(key, ClientWireApi::Responses) {
        protocols.push(WireApi::Responses);
    }
    if runtime.allows_client_wire_api(key, ClientWireApi::ChatCompletions) {
        protocols.push(WireApi::ChatCompletions);
    }
    protocols
}

fn allowed_codex_model_protocols(runtime: &GatewayRuntime, key: &AuthenticatedKey) -> Vec<WireApi> {
    if runtime.allows_client_wire_api(key, ClientWireApi::Responses) {
        vec![WireApi::Responses]
    } else {
        Vec::new()
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
    fn forwarded_messages_headers_keep_protocol_metadata_and_drop_client_credentials() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        client_headers.insert(
            "anthropic-beta",
            HeaderValue::from_static("fine-grained-tool"),
        );
        client_headers.insert(
            CLAUDE_CODE_SESSION_HEADER,
            HeaderValue::from_static("session-42"),
        );
        client_headers.insert("x-stainless-lang", HeaderValue::from_static("rust"));
        client_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer relay-local-secret"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("relay-local-secret"));
        client_headers.insert(
            "anthropic-api-key",
            HeaderValue::from_static("client-anthropic-secret"),
        );
        client_headers.insert(
            "openai-api-key",
            HeaderValue::from_static("client-openai-secret"),
        );
        client_headers.insert(
            "x-goog-api-key",
            HeaderValue::from_static("client-google-secret"),
        );
        client_headers.insert("cookie", HeaderValue::from_static("session=secret"));

        let forwarded = forwarded_messages_headers(&client_headers);

        assert_eq!(forwarded["anthropic-version"], "2023-06-01");
        assert_eq!(forwarded["anthropic-beta"], "fine-grained-tool");
        assert_eq!(forwarded[CLAUDE_CODE_SESSION_HEADER], "session-42");
        assert_eq!(forwarded["x-stainless-lang"], "rust");
        for name in [
            "authorization",
            "x-api-key",
            "anthropic-api-key",
            "openai-api-key",
            "x-goog-api-key",
            "cookie",
        ] {
            assert!(
                !forwarded.contains_key(name),
                "{name} must not be forwarded"
            );
        }
    }

    #[test]
    fn bridged_messages_headers_do_not_forward_codex_metadata() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("user-agent", HeaderValue::from_static("codex-test"));
        client_headers.insert(
            CLAUDE_CODE_SESSION_HEADER,
            HeaderValue::from_static("session-42"),
        );
        client_headers.insert(
            "x-oai-attestation",
            HeaderValue::from_static("private-attestation"),
        );
        client_headers.insert(
            "x-openai-memgen-request",
            HeaderValue::from_static("private-memgen"),
        );
        client_headers.insert("openai-beta", HeaderValue::from_static("responses=v1"));
        client_headers.insert("anthropic-beta", HeaderValue::from_static("tools"));

        let forwarded = forwarded_bridge_messages_headers(&client_headers);

        assert_eq!(forwarded["user-agent"], "codex-test");
        assert_eq!(forwarded[CLAUDE_CODE_SESSION_HEADER], "session-42");
        for name in [
            "x-oai-attestation",
            "x-openai-memgen-request",
            "openai-beta",
            "anthropic-beta",
        ] {
            assert!(
                !forwarded.contains_key(name),
                "{name} must not cross the Responses-to-Messages boundary"
            );
        }
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
    fn responses_lite_preserves_codex_client_tools_without_server_hosted_tools() {
        let mut request = json!({
            "model": "gpt-lite",
            "parallel_tool_calls": true,
            "tools": [
                {"type": "function", "name": "lookup"},
                {"type": "custom", "name": "patch"},
                {"type": "namespace", "name": "collaboration", "tools": [
                    {"type": "function", "name": "spawn_agent"},
                    {"type": "function", "name": "wait_agent"}
                ]},
                {"type": "tool_search"},
                {"type": "tool_search", "execution": "server"},
                {"type": "future_client_tool", "name": "future_tool"},
                {"type": "future_server_tool", "name": "hosted_tool", "execution": "server"},
                {"type": "web_search"},
                {"type": "web_search_preview_2025_03_11"},
                {"type": "image_generation"},
                {"type": "file_search"},
                {"type": "mcp", "name": "hosted_mcp"}
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "tools": [
                    {"type": "function", "name": "lookup"},
                    {"type": "namespace", "name": "collaboration"},
                    {"type": "tool_search"},
                    {"type": "future_client_tool", "name": "future_tool"},
                    {"type": "future_server_tool", "name": "hosted_tool", "execution": "server"},
                    {"type": "web_search"}
                ]
            },
            "input": [
                {"type": "additional_tools", "tools": [{"type": "web_search"}]},
                {"type": "additional_tools", "tools": [
                    {"type": "custom", "name": "patch"},
                    {"type": "image_generation"}
                ]},
                {"type": "additional_tools", "tools": [
                    {"type": "tool_search"},
                    {"type": "future_client_tool", "name": "future_tool"}
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
            [
                "function",
                "custom",
                "namespace",
                "tool_search",
                "future_client_tool"
            ]
        );
        assert_eq!(
            types("/tool_choice/tools"),
            ["function", "namespace", "tool_search", "future_client_tool"]
        );
        assert_eq!(types("/input/0/tools"), ["custom"]);
        assert_eq!(
            types("/input/1/tools"),
            ["tool_search", "future_client_tool"]
        );
        assert_eq!(types("/response/tools"), ["function"]);
        assert_eq!(request["input"].as_array().unwrap().len(), 3);
        assert!(request.pointer("/response/tool_choice").is_none());
        assert_eq!(request["parallel_tool_calls"], true);
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
    fn responses_lite_removes_choices_that_do_not_reference_remaining_client_tools() {
        let mut required = json!({
            "tools": [{"type": "web_search", "name": "server_only"}],
            "tool_choice": {"type": "function", "name": "server_only"}
        });
        normalize_account_request(required.as_object_mut().unwrap(), true);
        assert!(required
            .get("tools")
            .is_some_and(|tools| tools.as_array().is_some_and(Vec::is_empty)));
        assert!(required.get("tool_choice").is_none());

        let mut allowed = json!({
            "tools": [
                {"type": "function", "name": "client"},
                {"type": "namespace", "name": "collaboration", "tools": [
                    {"name": "spawn_agent"},
                    {"type": "web_search"}
                ]},
                {"type": "web_search", "name": "server_only"}
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "required",
                "tools": [
                    {"type": "function", "name": "client"},
                    {"type": "function", "name": "missing"},
                    {"type": "function", "namespace": "collaboration", "name": "spawn_agent"},
                    {"type": "web_search", "name": "server_only"}
                ]
            }
        });
        normalize_account_request(allowed.as_object_mut().unwrap(), true);

        assert_eq!(
            allowed["tools"][1]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["spawn_agent"]
        );
        assert_eq!(
            allowed["tool_choice"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["client", "spawn_agent"]
        );
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
    fn codex_catalog_uses_unique_priorities_and_preserves_confirmed_parallel_tools() {
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
                "GLM 5.2",
                "Grok 4.5",
            ]
        );
        assert!(models.iter().all(codex_catalog_entry_is_compatible));
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

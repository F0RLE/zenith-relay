use crate::accounts::CODEX_MODELS_CLIENT_VERSION;
use crate::runtime::{
    AuthenticatedKey, DefaultServiceTier, ExecutorPrepareError, ExecutorRoute, IMAGE_API_MODEL,
};
use crate::{Error, GatewayRuntime, UsageEvent, WireApi};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST, RETRY_AFTER, USER_AGENT,
    WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{stream, Stream, StreamExt};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod images;
mod websocket;

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CLIENT_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_CODEX_MODELS_BODY_BYTES: usize = 512 * 1024;
const MAX_ALPHA_SEARCH_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const CODEX_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const TRANSIENT_COOLDOWN_MS: u64 = 60_000;
const TRANSPORT_COOLDOWN_MS: u64 = 5_000;
const MAX_RESPONSE_OWNER_CANDIDATES: usize = 8;
const MAX_RATE_LIMIT_COOLDOWN_MS: u64 = 30 * 60_000;
const MAX_RATE_LIMIT_RETRY_HINT_MS: u64 = 7 * 24 * 60 * 60_000;
type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
type CompletionCallback =
    Arc<dyn Fn(&mut UsageEvent, Option<&str>, RateLimitBodyHint) + Send + Sync>;

pub fn router(runtime: Arc<GatewayRuntime>) -> Router {
    Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", get(websocket::responses).post(responses))
        .route("/v1/responses/compact", post(responses_compact))
        .route("/v1/chat/completions/v1/responses", post(responses))
        .route(
            "/v1/chat/completions/v1/responses/compact",
            post(responses_compact),
        )
        .route("/v1/alpha/search", post(alpha_search))
        .route("/backend-api/codex/alpha/search", post(alpha_search))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/images/generations", post(images::generations))
        .route("/v1/images/edits", post(images::edits))
        .with_state(runtime)
}

async fn models(
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
    let models = runtime.visible_models(
        &key,
        &[WireApi::Responses, WireApi::ChatCompletions],
        now_ms(),
    );
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
            codex_models_response(runtime.as_ref(), &key, &models, client_version).await
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
    visible_models: &[String],
    client_version: &str,
) -> Option<Value> {
    let now_ms = now_ms();
    let routes = runtime.codex_models_routes(key, now_ms).await;
    let client_versions = if client_version == CODEX_MODELS_CLIENT_VERSION {
        vec![client_version]
    } else {
        vec![client_version, CODEX_MODELS_CLIENT_VERSION]
    };
    for (candidate_id, mut url) in routes
        .into_iter()
        .take(runtime.max_retry_candidates().max(1))
    {
        let Ok(prepared) = runtime.prepare_authorization(&candidate_id, now_ms).await else {
            continue;
        };
        for client_version in &client_versions {
            url.query_pairs_mut()
                .clear()
                .append_pair("client_version", client_version);
            let mut request = runtime
                .request_client(&candidate_id, false)
                .get(url.clone())
                .header(AUTHORIZATION, prepared.authorization.clone())
                .timeout(Duration::from_secs(10));
            if let Some(account_id) = prepared.chatgpt_account_id.as_ref() {
                request = request.header("ChatGPT-Account-Id", account_id.clone());
            }
            if let Some(originator) = prepared.originator.as_ref() {
                request = request.header("originator", originator.clone());
            }
            let Ok(response) = request.send().await else {
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
            if let Some(catalog) = filter_codex_models_response(runtime, key, visible_models, &body)
            {
                return Some(catalog);
            }
        }
    }
    None
}

fn filter_codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    body: &[u8],
) -> Option<Value> {
    let payload: Value = serde_json::from_slice(body).ok()?;
    let models = payload.get("models")?.as_array()?;
    if models.len() > 4_096 {
        return None;
    }
    let visible = visible_models
        .iter()
        .filter_map(|display_id| {
            runtime
                .resolve_model(key, display_id)
                .map(|upstream_id| (upstream_id.to_ascii_lowercase(), display_id.clone()))
        })
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut models = models
        .iter()
        .filter_map(|model| {
            let mut model = model.as_object()?.clone();
            let slug = model.get("slug")?.as_str()?.trim();
            if slug.is_empty()
                || slug.len() > 256
                || slug.chars().any(char::is_control)
                || model.get("supported_in_api") == Some(&Value::Bool(false))
                || model
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("hide"))
            {
                return None;
            }
            let normalized = slug.to_ascii_lowercase();
            let display_id = visible.get(&normalized)?;
            if !seen.insert(normalized) {
                return None;
            }
            if model
                .get("use_responses_lite")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                runtime.remember_codex_responses_lite_model(slug);
                model.insert(
                    "supports_parallel_tool_calls".to_string(),
                    Value::Bool(false),
                );
            }
            model.insert("slug".to_string(), Value::String(display_id.clone()));
            Some(Value::Object(model))
        })
        .collect::<Vec<_>>();
    if let Some(display_id) = visible.get(IMAGE_API_MODEL.to_ascii_lowercase().as_str()) {
        if seen.insert(IMAGE_API_MODEL.to_string()) {
            models.push(json!({
                "slug": display_id,
                "display_name": "GPT Image 2",
                "visibility": "list",
                "supported_in_api": true,
                "supports_parallel_tool_calls": false
            }));
        }
    }
    (!models.is_empty()).then(|| json!({ "models": models }))
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

async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::Responses).await
}

async fn responses_compact(
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

async fn alpha_search(
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

async fn chat_completions(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::ChatCompletions).await
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AccountEndpoint {
    Compact,
    AlphaSearch,
}

impl AccountEndpoint {
    fn response_limit(self) -> usize {
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
                "request body exceeds 16 MiB",
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
    runtime
        .visible_account_models(key)
        .iter()
        .any(|model| model.eq_ignore_ascii_case(requested_model))
        .then(|| runtime.resolve_model(key, requested_model))
        .flatten()
}

#[allow(clippy::too_many_arguments)]
async fn execute_account_endpoint(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    request: Value,
    requested_model: String,
    resolved_model: String,
    client_headers: HeaderMap,
    endpoint: AccountEndpoint,
    responses_lite: Option<HeaderValue>,
    response_affinity_key: Option<String>,
    rewrite_model: bool,
) -> Response<Body> {
    let request_id = request_id();
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
    );
    let has_previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let account_only_exclusions = runtime.api_source_candidate_ids();
    let mut tried = account_only_exclusions.clone();
    let mut attempt = 0_u16;
    let mut owner_recovery_confirmed = false;
    let mut last_failure = None;

    while usize::from(attempt)
        < retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
    {
        let Some((selected, lease)) = runtime.select_and_reserve(
            &key,
            &resolved_model,
            &[WireApi::Responses],
            &tried,
            (
                response_affinity_key.as_deref(),
                prompt_affinity_key.as_deref(),
            ),
            now_ms(),
        ) else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) = runtime.executor_route(&selected.candidate_id, &resolved_model)
        else {
            continue;
        };
        if route.account_id.is_none() {
            continue;
        }
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        let Some(upstream_url) = account_endpoint_url(route.upstream_url.clone(), endpoint) else {
            last_failure = Some(AttemptFailure::invalid_request());
            continue;
        };
        let mut upstream_body = request.clone();
        if rewrite_model {
            upstream_body.as_object_mut().unwrap().insert(
                "model".to_string(),
                Value::String(route.source_model.clone()),
            );
        }
        let request_body = match serde_json::to_vec(&upstream_body) {
            Ok(body) => body,
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "request body could not be serialized",
                    "invalid_request",
                )
            }
        };

        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let prepared = match runtime
            .prepare_authorization(&route.candidate_id, now_ms())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure = AttemptFailure::prepare(error);
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    failure.cooldown_ms,
                    route.half_open_probe,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        let mut upstream_request = runtime
            .request_client(&route.candidate_id, false)
            .post(upstream_url)
            .header(AUTHORIZATION, prepared.authorization)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json");
        if let Some(account_id) = prepared.chatgpt_account_id {
            upstream_request = upstream_request.header("ChatGPT-Account-Id", account_id);
        }
        if let Some(originator) = prepared.originator {
            upstream_request = upstream_request.header("originator", originator);
        }
        if endpoint == AccountEndpoint::Compact {
            if let Some(value) = responses_lite.as_ref() {
                upstream_request =
                    upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value.clone());
            }
        } else {
            for name in [
                USER_AGENT.as_str(),
                "version",
                "session_id",
                "x-session-id",
                "x-client-request-id",
                "x-openai-actor-authorization",
            ] {
                if let Some(value) = client_headers.get(name) {
                    upstream_request = upstream_request.header(name, value.clone());
                }
            }
        }
        let upstream = upstream_request.body(request_body).send().await;
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(error) => {
                let failure = AttemptFailure::transport(&error);
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    failure.cooldown_ms,
                    route.half_open_probe,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        let bytes = match crate::runtime::collect_limited(upstream, endpoint.response_limit()).await
        {
            Ok(bytes) => bytes,
            Err(_) => {
                let failure = AttemptFailure::body();
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    TRANSIENT_COOLDOWN_MS,
                    route.half_open_probe,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        if !status.is_success() {
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                false,
                status.as_u16(),
                Some(failure.category.to_string()),
                started.elapsed().as_millis() as u64,
            );
            let affinity_miss = recoverable_response_affinity_miss(
                status,
                has_previous_response_id,
                response_affinity_hit,
                previous_response_not_found(&bytes),
            );
            if affinity_miss
                || retryable_failure(status, failure.category, has_previous_response_id)
            {
                if affinity_miss {
                    owner_recovery_confirmed |= !response_affinity_hit;
                    runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                    event.error_category = Some("response_affinity_miss".to_string());
                } else {
                    let state = apply_failure_cooldown_with_body(
                        &runtime,
                        &route.candidate_id,
                        &route.source_model,
                        status,
                        failure.category,
                        &response_headers,
                        Some(&bytes),
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                }
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
            emit_usage(&runtime, event);
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

        let mut event = usage_event(
            &request_id,
            attempt,
            &key.id,
            &route,
            &requested_model,
            true,
            status.as_u16(),
            None,
            started.elapsed().as_millis() as u64,
        );
        populate_tokens(&mut event, &bytes);
        let recovered = runtime.record_success_with_metrics(
            &route.candidate_id,
            &route.source_model,
            now_ms(),
            event.output_tokens,
            event.generation_ms.unwrap_or(event.latency_ms),
        );
        event.consecutive_failures = recovered.then_some(0);
        runtime.bind_prompt_affinity(
            prompt_affinity_key.as_deref(),
            &route.candidate_id,
            now_ms(),
        );
        emit_usage(&runtime, event);
        drop(lease);
        return proxy_response(status, &response_headers, Body::from(bytes));
    }

    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(retry_at) = runtime.earliest_retry_at(
            &key,
            &resolved_model,
            &[WireApi::Responses],
            &account_only_exclusions,
            response_affinity_key.as_deref(),
            now_ms(),
        ) {
            return cooldown_error(retry_at);
        }
    }
    api_error(failure.status, failure.message, failure.category)
}

fn account_endpoint_url(
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

async fn execute_client_request(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    wire_api: WireApi,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    let body = match axum::body::to_bytes(body, MAX_CLIENT_REQUEST_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds 16 MiB",
                "request_too_large",
            )
        }
    };

    let mut request: Value = match serde_json::from_slice(&body) {
        Ok(Value::Object(request)) => Value::Object(request),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
                "invalid_request",
            )
        }
    };
    if let Some(object) = request.as_object_mut() {
        normalize_service_tier(object, runtime.default_service_tier());
    }
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
    let stream = match request.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                "invalid_request",
            )
        }
        None => false,
    };
    let visible_models = runtime.visible_models(&key, candidate_protocols(wire_api), now_ms());
    if !visible_models
        .iter()
        .any(|model| model.eq_ignore_ascii_case(&requested_model))
    {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    }
    let Some(resolved_model) = runtime.resolve_model(&key, &requested_model) else {
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
    execute_request(
        runtime,
        key,
        request,
        requested_model,
        resolved_model,
        stream,
        request_id(),
        response_affinity_key,
        wire_api,
        responses_lite,
        true,
        0,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_request(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    request: Value,
    requested_model: String,
    resolved_model: String,
    stream: bool,
    request_id: String,
    response_affinity_key: Option<String>,
    wire_api: WireApi,
    responses_lite: Option<HeaderValue>,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
) -> Response<Body> {
    let mut tried = HashSet::new();
    let mut attempt = attempt_offset;
    let mut attempts_this_run = 0_usize;
    let mut owner_recovery_confirmed = false;
    let mut confirmed_response_missing = false;
    let mut last_failure = None;
    let has_previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
    );

    while attempts_this_run
        < retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
    {
        let selected = runtime.select_and_reserve(
            &key,
            &resolved_model,
            candidate_protocols(wire_api),
            &tried,
            (
                response_affinity_key.as_deref(),
                prompt_affinity_key.as_deref(),
            ),
            now_ms(),
        );
        let Some((selected, lease)) = selected else {
            if attempt == 0 {
                if let Some(retry_at) = runtime.earliest_retry_at(
                    &key,
                    &resolved_model,
                    candidate_protocols(wire_api),
                    &tried,
                    response_affinity_key.as_deref(),
                    now_ms(),
                ) {
                    return cooldown_error(retry_at);
                }
            }
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) = runtime.executor_route(&selected.candidate_id, &resolved_model)
        else {
            continue;
        };
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        let source_model = route.source_model.clone();
        let responses_via_chat =
            wire_api == WireApi::Responses && route.wire_api == WireApi::ChatCompletions;
        let chat_via_responses =
            wire_api == WireApi::ChatCompletions && route.wire_api == WireApi::Responses;
        let account_route = route.account_id.is_some();
        let request_body = if responses_via_chat {
            match translate_responses_request(&request, &source_model, false) {
                Ok(body) => body,
                Err(failure) => {
                    last_failure = Some(failure);
                    continue;
                }
            }
        } else if chat_via_responses {
            match translate_chat_request(&request, &source_model, false) {
                Ok(body) if account_route => {
                    match normalize_account_request_body(&body, responses_lite.is_some()) {
                        Ok(body) => body,
                        Err(failure) => {
                            last_failure = Some(failure);
                            continue;
                        }
                    }
                }
                Ok(body) => body,
                Err(failure) => {
                    last_failure = Some(failure);
                    continue;
                }
            }
        } else {
            let mut upstream_request = request.clone();
            let Value::Object(object) = &mut upstream_request else {
                unreachable!("request object was validated before execution")
            };
            object.insert("model".to_string(), Value::String(source_model.clone()));
            if account_route {
                normalize_account_request(object, responses_lite.is_some());
            }
            match serde_json::to_vec(&upstream_request) {
                Ok(body) => body,
                Err(_) => {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "request body could not be serialized",
                        "invalid_request",
                    )
                }
            }
        };

        // ponytail: cross-protocol streams are buffered into one terminal SSE sequence; add delta translation when adapter TTFT matters.
        let upstream_stream = stream && !responses_via_chat && !chat_via_responses;
        attempt = attempt.saturating_add(1);
        attempts_this_run = attempts_this_run.saturating_add(1);
        let started = Instant::now();
        let prepared = match runtime
            .prepare_authorization(&route.candidate_id, now_ms())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure = AttemptFailure::prepare(error);
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    failure.cooldown_ms,
                    route.half_open_probe,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        let client = runtime.request_client(&route.candidate_id, upstream_stream);
        let mut upstream_request = client
            .post(route.upstream_url.clone())
            .header(AUTHORIZATION, prepared.authorization)
            .header(CONTENT_TYPE, "application/json");
        if upstream_stream {
            upstream_request = upstream_request.header(ACCEPT, "text/event-stream");
        }
        if let Some(account_id) = prepared.chatgpt_account_id {
            upstream_request = upstream_request.header("ChatGPT-Account-Id", account_id);
        }
        if let Some(originator) = prepared.originator {
            upstream_request = upstream_request.header("originator", originator);
        }
        if account_route {
            if let Some(value) = responses_lite.as_ref() {
                upstream_request = upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value);
            }
        }
        let upstream = upstream_request.body(request_body).send().await;
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(error) => {
                let failure = AttemptFailure::transport(&error);
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    failure.cooldown_ms,
                    route.half_open_probe,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };

        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        if !status.is_success() {
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                false,
                status.as_u16(),
                None,
                started.elapsed().as_millis() as u64,
            );
            let bytes = match crate::runtime::collect_limited(
                upstream,
                crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(_) if retryable_status(status, has_previous_response_id) => {
                    let failure = AttemptFailure::status_with_body(status, None);
                    event.error_category = Some(failure.category.to_string());
                    let state = apply_failure_cooldown_with_body(
                        &runtime,
                        &route.candidate_id,
                        &source_model,
                        status,
                        failure.category,
                        &response_headers,
                        None,
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    continue;
                }
                Err(error) => return upstream_body_error_response(&runtime, event, started, error),
            };
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            event.error_category = Some(failure.category.to_string());
            let response_missing = previous_response_not_found(&bytes);
            let affinity_miss = recoverable_response_affinity_miss(
                status,
                has_previous_response_id,
                response_affinity_hit,
                response_missing,
            );
            if affinity_miss
                || retryable_failure(status, failure.category, has_previous_response_id)
            {
                if affinity_miss {
                    confirmed_response_missing |= response_missing;
                    owner_recovery_confirmed |= !response_affinity_hit;
                    runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                    event.error_category = Some("response_affinity_miss".to_string());
                } else {
                    let state = apply_failure_cooldown_with_body(
                        &runtime,
                        &route.candidate_id,
                        &source_model,
                        status,
                        failure.category,
                        &response_headers,
                        Some(&bytes),
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                }
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                if affinity_miss && response_missing && response_affinity_hit {
                    break;
                }
                continue;
            }
            populate_tokens(&mut event, &bytes);
            emit_usage(&runtime, event);
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

        if !upstream_stream {
            let bytes = match crate::runtime::collect_limited(
                upstream,
                crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    let too_large = matches!(error, Error::UpstreamBodyTooLarge);
                    let state = apply_cooldown(
                        &runtime,
                        &route.candidate_id,
                        "*",
                        TRANSIENT_COOLDOWN_MS,
                        route.half_open_probe,
                    );
                    let mut event = usage_event(
                        &request_id,
                        attempt,
                        &key.id,
                        &route,
                        &requested_model,
                        false,
                        StatusCode::BAD_GATEWAY.as_u16(),
                        Some(if too_large {
                            "upstream_body_too_large".to_string()
                        } else {
                            "upstream_body".to_string()
                        }),
                        started.elapsed().as_millis() as u64,
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    last_failure = Some(AttemptFailure::body());
                    continue;
                }
            };
            let bytes = if account_route {
                match completed_account_response(&bytes) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state =
                            failure_category_requires_cooldown(failure.category).then(|| {
                                apply_attempt_failure_cooldown(
                                    &runtime,
                                    &route.candidate_id,
                                    &source_model,
                                    &failure,
                                    &response_headers,
                                    route.half_open_probe,
                                )
                            });
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            &requested_model,
                            false,
                            failure.status.as_u16(),
                            Some(failure.category.to_string()),
                            started.elapsed().as_millis() as u64,
                        );
                        if let Some(state) = state {
                            apply_failure_state(&mut event, state);
                        }
                        emit_usage(&runtime, event);
                        if failure_category_is_request_terminal(failure.category) {
                            return api_error(failure.status, failure.message, failure.category);
                        }
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else {
                bytes
            };
            let bytes = if responses_via_chat {
                match translate_chat_response(&bytes) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state =
                            failure_category_requires_cooldown(failure.category).then(|| {
                                apply_attempt_failure_cooldown(
                                    &runtime,
                                    &route.candidate_id,
                                    &source_model,
                                    &failure,
                                    &response_headers,
                                    route.half_open_probe,
                                )
                            });
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            &requested_model,
                            false,
                            failure.status.as_u16(),
                            Some(failure.category.to_string()),
                            started.elapsed().as_millis() as u64,
                        );
                        if let Some(state) = state {
                            apply_failure_state(&mut event, state);
                        }
                        emit_usage(&runtime, event);
                        if failure_category_is_request_terminal(failure.category) {
                            return api_error(failure.status, failure.message, failure.category);
                        }
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else if chat_via_responses {
                match translate_responses_response(&bytes) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state =
                            failure_category_requires_cooldown(failure.category).then(|| {
                                apply_attempt_failure_cooldown(
                                    &runtime,
                                    &route.candidate_id,
                                    &source_model,
                                    &failure,
                                    &response_headers,
                                    route.half_open_probe,
                                )
                            });
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            &requested_model,
                            false,
                            failure.status.as_u16(),
                            Some(failure.category.to_string()),
                            started.elapsed().as_millis() as u64,
                        );
                        if let Some(state) = state {
                            apply_failure_state(&mut event, state);
                        }
                        emit_usage(&runtime, event);
                        if failure_category_is_request_terminal(failure.category) {
                            return api_error(failure.status, failure.message, failure.category);
                        }
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else {
                bytes
            };
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                true,
                status.as_u16(),
                None,
                started.elapsed().as_millis() as u64,
            );
            populate_tokens(&mut event, &bytes);
            let recovered = runtime.record_success_with_metrics(
                &route.candidate_id,
                &source_model,
                now_ms(),
                event.output_tokens,
                event.generation_ms.unwrap_or(event.latency_ms),
            );
            event.consecutive_failures = recovered.then_some(0);
            runtime.bind_prompt_affinity(
                prompt_affinity_key.as_deref(),
                &route.candidate_id,
                now_ms(),
            );
            emit_usage(&runtime, event);
            let completed_response_id = response_id_from_bytes(&bytes);
            runtime.bind_response_affinity(
                completed_response_id.as_deref(),
                &route.candidate_id,
                now_ms(),
            );
            if stream {
                let body = match wire_api {
                    WireApi::Responses => completed_sse(&bytes),
                    WireApi::ChatCompletions => completed_chat_sse(&bytes),
                    WireApi::Messages => Bytes::new(),
                };
                return proxy_sse_response(status, &response_headers, Body::from(body));
            }
            if account_route {
                return proxy_json_response(status, &response_headers, Body::from(bytes));
            }
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

        match bootstrap_stream(upstream).await {
            Ok((headers, first, remaining)) => {
                let completion_runtime = runtime.clone();
                let completion_source = route.candidate_id.clone();
                let completion_model = source_model.clone();
                let completion_prompt_affinity = prompt_affinity_key.clone();
                let completion_half_open_probe = route.half_open_probe;
                let completion_headers = headers.clone();
                let completion: CompletionCallback = Arc::new(move |event, response_id, hint| {
                    lease.release();
                    if event.success {
                        let recovered = completion_runtime.record_success_with_metrics(
                            &completion_source,
                            &completion_model,
                            now_ms(),
                            event.output_tokens,
                            event.generation_ms.unwrap_or(event.latency_ms),
                        );
                        event.consecutive_failures = recovered.then_some(0);
                        completion_runtime.bind_prompt_affinity(
                            completion_prompt_affinity.as_deref(),
                            &completion_source,
                            now_ms(),
                        );
                        completion_runtime.bind_response_affinity(
                            response_id,
                            &completion_source,
                            now_ms(),
                        );
                    } else if let Some(category) = event
                        .error_category
                        .as_deref()
                        .filter(|category| failure_category_requires_cooldown(category))
                    {
                        let status = StatusCode::from_u16(event.http_status)
                            .unwrap_or(StatusCode::BAD_GATEWAY);
                        let state = apply_failure_cooldown_with_hint(
                            &completion_runtime,
                            &completion_source,
                            &completion_model,
                            status,
                            category,
                            &completion_headers,
                            hint,
                            completion_half_open_probe,
                        );
                        apply_failure_state(event, state);
                    }
                });
                let combined =
                    stream::once(async move { Ok::<_, reqwest::Error>(first) }).chain(remaining);
                let usage_stream = UsageStream::new(
                    combined,
                    runtime.usage.clone(),
                    usage_event(
                        &request_id,
                        attempt,
                        &key.id,
                        &route,
                        &requested_model,
                        true,
                        status.as_u16(),
                        None,
                        0,
                    ),
                    started,
                    completion,
                );
                return proxy_sse_response(status, &headers, Body::from_stream(usage_stream));
            }
            Err(failure) => {
                let state = failure_category_requires_cooldown(failure.category).then(|| {
                    apply_attempt_failure_cooldown(
                        &runtime,
                        &route.candidate_id,
                        &source_model,
                        &failure,
                        &response_headers,
                        route.half_open_probe,
                    )
                });
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                if let Some(state) = state {
                    apply_failure_state(&mut event, state);
                }
                emit_usage(&runtime, event);
                if failure_category_is_request_terminal(failure.category) {
                    return api_error(failure.status, failure.message, failure.category);
                }
                last_failure = Some(failure);
            }
        }
    }

    if allow_previous_response_reset
        && has_previous_response_id
        && confirmed_response_missing
        && !contains_function_call_output(&request)
    {
        let mut reset_request = request;
        if let Some(object) = reset_request.as_object_mut() {
            object.remove("previous_response_id");
            return Box::pin(execute_request(
                runtime,
                key,
                reset_request,
                requested_model,
                resolved_model,
                stream,
                request_id,
                None,
                wire_api,
                responses_lite,
                false,
                attempt,
            ))
            .await;
        }
    }

    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(retry_at) = runtime.earliest_retry_at(
            &key,
            &resolved_model,
            candidate_protocols(wire_api),
            &HashSet::new(),
            response_affinity_key.as_deref(),
            now_ms(),
        ) {
            return cooldown_error(retry_at);
        }
    }
    api_error(failure.status, failure.message, failure.category)
}

fn upstream_body_error_response(
    runtime: &GatewayRuntime,
    mut event: UsageEvent,
    started: Instant,
    error: Error,
) -> Response<Body> {
    event.success = false;
    event.http_status = StatusCode::BAD_GATEWAY.as_u16();
    let too_large = matches!(error, Error::UpstreamBodyTooLarge);
    event.error_category = Some(if too_large {
        "upstream_body_too_large".to_string()
    } else {
        "upstream_body".to_string()
    });
    event.latency_ms = started.elapsed().as_millis() as u64;
    emit_usage(runtime, event);
    api_error(
        StatusCode::BAD_GATEWAY,
        if too_large {
            "upstream response is too large"
        } else {
            "upstream response failed"
        },
        "upstream_error",
    )
}

async fn bootstrap_stream(
    upstream: reqwest::Response,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), AttemptFailure> {
    let headers = upstream.headers().clone();
    let mut stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    let mut buffer = Vec::new();
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if buffer.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream("stream_event_too_large"));
                }
                buffer.extend_from_slice(&chunk);
                let mut inspected = 0;
                while let Some(end) = sse_event_end(&buffer[inspected..]) {
                    let absolute_end = inspected + end;
                    let event = parse_sse_event(&buffer[inspected..absolute_end]);
                    if event.has_data && !event.valid {
                        return Err(AttemptFailure::stream("stream_invalid"));
                    }
                    if event.outcome == Some(TerminalOutcome::Failure) {
                        let category = event.error_category.unwrap_or("upstream_terminal");
                        return Err(AttemptFailure::classified_with_hint(
                            event
                                .error_status
                                .unwrap_or_else(|| upstream_failure_status(category)),
                            category,
                            event.cooldown_hint,
                        ));
                    }
                    if event.has_data {
                        return Ok((headers, Bytes::from(buffer), stream));
                    }
                    inspected = absolute_end;
                }
            }
            Some(Err(error)) => return Err(AttemptFailure::transport(&error)),
            None => return Err(AttemptFailure::stream("stream_incomplete")),
        }
    }
}

fn translate_responses_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    if object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(AttemptFailure::invalid_request());
    }

    let mut messages = Vec::new();
    if let Some(instructions) = object.get("instructions") {
        let Some(instructions) = instructions.as_str() else {
            return Err(AttemptFailure::invalid_request());
        };
        if !instructions.trim().is_empty() {
            messages.push(json!({"role": "system", "content": instructions}));
        }
    }
    let input = object
        .get("input")
        .ok_or_else(AttemptFailure::invalid_request)?;
    match input {
        Value::String(text) => messages.push(json!({"role": "user", "content": text})),
        Value::Array(items)
            if !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.get("role").and_then(Value::as_str).is_some()) =>
        {
            for item in items {
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                if !matches!(role, "developer" | "system" | "user" | "assistant" | "tool") {
                    return Err(AttemptFailure::invalid_request());
                }
                let content = translate_message_content(
                    item.get("content")
                        .ok_or_else(AttemptFailure::invalid_request)?,
                )?;
                messages.push(json!({"role": role, "content": content}));
            }
        }
        Value::Array(items) if !items.is_empty() => messages.push(json!({
            "role": "user",
            "content": translate_message_content(input)?,
        })),
        _ => return Err(AttemptFailure::invalid_request()),
    }

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("messages".to_string(), Value::Array(messages)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in [
        "temperature",
        "top_p",
        "stop",
        "parallel_tool_calls",
        "service_tier",
    ] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object.get("max_output_tokens") {
        translated.insert("max_completion_tokens".to_string(), value.clone());
    }
    if let Some(tools) = object.get("tools") {
        translated.insert("tools".to_string(), translate_tools(tools)?);
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        translated.insert(
            "tool_choice".to_string(),
            translate_tool_choice(tool_choice)?,
        );
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

fn translate_chat_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(AttemptFailure::invalid_request)?;
    let mut input = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(AttemptFailure::invalid_request)?;
        if role == "tool" {
            let call_id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let output = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            input.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
            continue;
        }
        if !matches!(role, "developer" | "system" | "user" | "assistant") {
            return Err(AttemptFailure::invalid_request());
        }
        if let Some(content) = message.get("content").filter(|content| !content.is_null()) {
            input.push(json!({
                "role": role,
                "content": translate_chat_message_content(content)?,
            }));
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                let function = call
                    .get("function")
                    .and_then(Value::as_object)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.get("id").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "name": function.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "arguments": function.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                }));
            }
        }
    }
    if input.is_empty() {
        return Err(AttemptFailure::invalid_request());
    }

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("input".to_string(), Value::Array(input)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in [
        "temperature",
        "top_p",
        "parallel_tool_calls",
        "service_tier",
    ] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object
        .get("max_completion_tokens")
        .or_else(|| object.get("max_tokens"))
    {
        translated.insert("max_output_tokens".to_string(), value.clone());
    }
    if let Some(tools) = object.get("tools") {
        translated.insert("tools".to_string(), translate_chat_tools(tools)?);
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        translated.insert(
            "tool_choice".to_string(),
            translate_chat_tool_choice(tool_choice)?,
        );
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

fn normalize_account_request_body(
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

fn normalize_account_request(object: &mut serde_json::Map<String, Value>, responses_lite: bool) {
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    object.remove("max_output_tokens");
    sanitize_unstored_reasoning_items(object);
    if responses_lite {
        object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
        filter_responses_lite_tools(object);
    }
    if let Some(Value::String(text)) = object.get("input") {
        let text = text.clone();
        object.insert(
            "input".to_string(),
            json!([{"role": "user", "content": [{"type": "input_text", "text": text}]}]),
        );
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
    if tool_type.eq_ignore_ascii_case("function") || tool_type.eq_ignore_ascii_case("custom") {
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
    if choice_type.eq_ignore_ascii_case("function") || choice_type.eq_ignore_ascii_case("custom") {
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

fn translate_chat_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({"type": "input_text", "text": text}))
                    .ok_or_else(AttemptFailure::invalid_request),
                Some("image_url") => item
                    .get("image_url")
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(|url| json!({"type": "input_image", "image_url": url}))
                    .ok_or_else(AttemptFailure::invalid_request),
                _ => Err(AttemptFailure::invalid_request()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_chat_tools(tools: &Value) -> Result<Value, AttemptFailure> {
    let tools = tools
        .as_array()
        .ok_or_else(AttemptFailure::invalid_request)?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AttemptFailure::invalid_request());
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let mut translated = serde_json::Map::from_iter([
                ("type".to_string(), Value::String("function".to_string())),
                (
                    "name".to_string(),
                    function
                        .get("name")
                        .cloned()
                        .ok_or_else(AttemptFailure::invalid_request)?,
                ),
            ]);
            for field in ["description", "parameters", "strict"] {
                if let Some(value) = function.get(field) {
                    translated.insert(field.to_string(), value.clone());
                }
            }
            Ok(Value::Object(translated))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn translate_chat_tool_choice(tool_choice: &Value) -> Result<Value, AttemptFailure> {
    if tool_choice.is_string() {
        return Ok(tool_choice.clone());
    }
    let name = tool_choice
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(json!({"type": "function", "name": name}))
}

fn translate_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if let Some(text) = item.as_str() {
                    return Ok(json!({"type": "text", "text": text}));
                }
                let kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                match kind {
                    "input_text" | "output_text" | "text" => item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| json!({"type": "text", "text": text}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    "input_image" => item
                        .get("image_url")
                        .or_else(|| item.get("url"))
                        .and_then(Value::as_str)
                        .map(|url| json!({"type": "image_url", "image_url": {"url": url}}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    _ => Err(AttemptFailure::invalid_request()),
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_tools(tools: &Value) -> Result<Value, AttemptFailure> {
    let tools = tools
        .as_array()
        .ok_or_else(AttemptFailure::invalid_request)?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AttemptFailure::invalid_request());
            }
            if let Some(function) = tool.get("function") {
                return Ok(json!({"type": "function", "function": function}));
            }
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let mut function =
                serde_json::Map::from_iter([("name".to_string(), Value::String(name.to_string()))]);
            for field in ["description", "parameters", "strict"] {
                if let Some(value) = tool.get(field) {
                    function.insert(field.to_string(), value.clone());
                }
            }
            Ok(json!({"type": "function", "function": function}))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn translate_tool_choice(tool_choice: &Value) -> Result<Value, AttemptFailure> {
    if tool_choice.is_string() {
        return Ok(tool_choice.clone());
    }
    let name = tool_choice
        .get("name")
        .or_else(|| {
            tool_choice
                .get("function")
                .and_then(|value| value.get("name"))
        })
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(json!({"type": "function", "function": {"name": name}}))
}

fn translate_chat_response(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(AttemptFailure::translation)?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(AttemptFailure::translation)?;
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chat-response");
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut output = Vec::new();
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        output.push(json!({
            "id": format!("{id}-message"),
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": content, "annotations": []}],
        }));
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        let choice_index = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
        for (tool_index, call) in tool_calls.iter().enumerate() {
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::translation)?;
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{id}_{choice_index}_{tool_index}"));
            output.push(json!({
                "id": call_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": function.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::translation)?,
                "arguments": function.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
            }));
        }
    }
    if output.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "input_tokens": usage.get("prompt_tokens").or_else(|| usage.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": usage.get("completion_tokens").or_else(|| usage.get("output_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": id,
        "object": "response",
        "created_at": response.get("created").and_then(Value::as_u64).unwrap_or(0),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

fn translate_responses_response(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(AttemptFailure::translation)?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if matches!(
                            part.get("type").and_then(Value::as_str),
                            Some("output_text" | "text")
                        ) {
                            if let Some(value) = part.get("text").and_then(Value::as_str) {
                                text.push_str(value);
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::translation)?;
                tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": item.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::translation)?,
                        "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                    }
                }));
            }
            _ => {}
        }
    }
    if text.is_empty() && tool_calls.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let mut message = serde_json::Map::from_iter([
        ("role".to_string(), Value::String("assistant".to_string())),
        (
            "content".to_string(),
            if text.is_empty() {
                Value::Null
            } else {
                Value::String(text)
            },
        ),
    ]);
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "prompt_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": response.get("id").and_then(Value::as_str).unwrap_or("response"),
        "object": "chat.completion",
        "created": response.get("created_at").and_then(Value::as_u64).unwrap_or(0),
        "model": response.get("model").and_then(Value::as_str).unwrap_or("unknown"),
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": if response.get("status").and_then(Value::as_str) == Some("incomplete") { "length" } else if response.get("output").and_then(Value::as_array).is_some_and(|output| output.iter().any(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))) { "tool_calls" } else { "stop" },
        }],
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

fn completed_sse(response: &[u8]) -> Bytes {
    let response = serde_json::from_slice::<Value>(response).unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"type": "response.completed", "response": response})
    ))
}

fn completed_chat_sse(response: &[u8]) -> Bytes {
    let Ok(response) = serde_json::from_slice::<Value>(response) else {
        return Bytes::new();
    };
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let common = json!({
        "id": response.get("id").cloned().unwrap_or(Value::Null),
        "object": "chat.completion.chunk",
        "created": response.get("created").cloned().unwrap_or(Value::from(0)),
        "model": response.get("model").cloned().unwrap_or(Value::Null),
    });
    let mut first = common.clone();
    first["choices"] = json!([{
        "index": 0,
        "delta": choice.get("message").cloned().unwrap_or(Value::Null),
        "finish_reason": Value::Null,
    }]);
    let mut terminal = common;
    terminal["choices"] = json!([{
        "index": 0,
        "delta": {},
        "finish_reason": choice.get("finish_reason").cloned().unwrap_or(Value::String("stop".to_string())),
    }]);
    terminal["usage"] = response.get("usage").cloned().unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {first}\n\ndata: {terminal}\n\ndata: [DONE]\n\n"
    ))
}

#[derive(Clone, Copy)]
struct AttemptFailure {
    status: StatusCode,
    category: &'static str,
    message: &'static str,
    cooldown_ms: u64,
    cooldown_hint: RateLimitBodyHint,
}

struct FailureState {
    cooldown_scope: String,
    retry_at_ms: u64,
    consecutive_failures: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UpstreamErrorClassification {
    category: &'static str,
    message: &'static str,
}

fn classify_upstream_error(status: StatusCode, body: Option<&[u8]>) -> UpstreamErrorClassification {
    let Some(body) = body else {
        return classify_upstream_error_text(status, "");
    };
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => classify_upstream_error_value(status, &value),
        Err(_) => classify_upstream_error_text(status, &normalized_error_text(body)),
    }
}

fn classify_upstream_error_value(status: StatusCode, value: &Value) -> UpstreamErrorClassification {
    classify_upstream_error_text(status, &upstream_error_text(value))
}

fn classify_upstream_error_text(status: StatusCode, text: &str) -> UpstreamErrorClassification {
    let category = if text_has_any(
        text,
        &[
            "previous_response_not_found",
            "invalid_previous_response_id",
            "previous response not found",
            "no response found for previous_response_id",
            "unknown or expired previous_response_id",
        ],
    ) {
        "upstream_previous_response_not_found"
    } else if text_has_any(
        text,
        &[
            "tool_call_not_found",
            "no tool call found for",
            "no matching tool call",
            "tool call output does not match",
            "unanswered_function_call",
            "no tool output found for function call",
            "no tool output found for custom tool call",
            "no tool output found for apply patch call",
        ],
    ) {
        "upstream_tool_call_mismatch"
    } else if text_has_any(
        text,
        &[
            "context_length_exceeded",
            "context_window_exceeded",
            "context_too_large",
            "maximum context length",
            "max context length",
        ],
    ) || (text.contains("context window")
        && text_has_any(text, &["exceed", "too large", "too long"]))
        || (text.contains("context length")
            && text_has_any(text, &["exceed", "too large", "too long"]))
    {
        "upstream_context_too_large"
    } else if text_has_any(
        text,
        &[
            "invalid_encrypted_content",
            "thinking_signature_invalid",
            "invalid signature in thinking block",
            "encrypted content could not be verified",
        ],
    ) {
        "upstream_encrypted_content_invalid"
    } else if text_has_any(
        text,
        &[
            "instructions are required",
            "required parameter: 'instructions'",
            "required parameter: instructions",
        ],
    ) {
        "upstream_instructions_required"
    } else if text_has_any(
        text,
        &[
            "account_deactivated",
            "account_disabled",
            "account_expired",
            "organization_deactivated",
            "organization_disabled",
            "project_deactivated",
            "deactivated_workspace",
            "workspace_disabled",
            "workspace_expired",
            "workspace_terminated",
            "account has been deactivated",
            "account is disabled",
        ],
    ) {
        "upstream_account_disabled"
    } else if text_has_any(
        text,
        &[
            "usage_not_included",
            "not included in your plan",
            "subscription does not include",
        ],
    ) {
        "upstream_usage_not_included"
    } else if text_has_any(
        text,
        &[
            "insufficient_quota",
            "usage_limit_reached",
            "usage_limit_exceeded",
            "usage limit reached",
            "quota_exhausted",
            "quota exceeded",
            "billing_hard_limit_reached",
            "credit_balance_exhausted",
            "credits_exhausted",
            "credits exhausted",
            "exceeded your current quota",
            "out of credits",
            "add credits to continue",
        ],
    ) || status == StatusCode::PAYMENT_REQUIRED
    {
        "upstream_quota_exhausted"
    } else if text_has_any(
        text,
        &[
            "invalid_api_key",
            "authentication_error",
            "invalid authentication",
            "invalid bearer token",
            "expired_token",
            "token_expired",
            "token_invalidated",
            "token_revoked",
            "refresh_token_reused",
            "invalid or expired token",
            "invalid_grant",
        ],
    ) || status == StatusCode::UNAUTHORIZED
    {
        "upstream_unauthorized"
    } else if text_has_any(
        text,
        &[
            "unsupported_country_region_territory",
            "country_not_supported",
            "region_not_supported",
            "country, region, or territory not supported",
        ],
    ) {
        "upstream_region_unsupported"
    } else if text_has_any(
        text,
        &[
            "content_policy_violation",
            "content_filter",
            "policy_violation",
            "safety_violation",
            "cyber_policy",
            "bio_policy",
            "content_moderation_failed",
        ],
    ) {
        "upstream_content_policy"
    } else if text_has_any(text, &["invalid_prompt"]) {
        "upstream_invalid_request"
    } else if status == StatusCode::PAYLOAD_TOO_LARGE
        || text_has_any(
            text,
            &[
                "request_too_large",
                "payload_too_large",
                "content_too_large",
                "request body too large",
                "length limit exceeded",
            ],
        )
    {
        "upstream_payload_too_large"
    } else if text_has_any(
        text,
        &[
            "unsupported_parameter",
            "unsupported_value",
            "invalid_parameter",
            "parameter_not_supported",
        ],
    ) {
        "upstream_unsupported_request"
    } else if text_has_any(
        text,
        &[
            "model_at_capacity",
            "selected model is at capacity",
            "model is at capacity",
        ],
    ) {
        "upstream_model_capacity"
    } else if text_has_any(text, &["model_not_found", "model_not_available"]) {
        "upstream_model_not_found"
    } else if status == StatusCode::NOT_ACCEPTABLE
        || text_has_any(
            text,
            &[
                "model_not_supported",
                "requested model is not supported",
                "model is not supported when using codex with a chatgpt account",
                "is not currently available for this chatgpt account",
            ],
        )
        || (text.contains("model")
            && text.contains("does not exist or you do not have access to it"))
    {
        "upstream_model_unsupported"
    } else if status == StatusCode::UPGRADE_REQUIRED
        || text_has_any(text, &["websocket_not_supported", "websocket_unsupported"])
    {
        "upstream_websocket_unsupported"
    } else if text.contains("websocket_connection_limit_reached") {
        "upstream_websocket_connection_limit"
    } else if text_has_any(
        text,
        &[
            "rate_limit_exceeded",
            "rate_limit_error",
            "rate_limit_reached",
            "rate limit reached",
            "rate limit exceeded",
            "too many requests",
        ],
    ) {
        "upstream_rate_limited"
    } else if status.as_u16() == 529
        || text_has_any(
            text,
            &[
                "server_is_overloaded",
                "server_overloaded",
                "overloaded",
                "slow_down",
                "slow down",
            ],
        )
    {
        "upstream_overloaded"
    } else if text_has_any(text, &["service_unavailable", "temporarily unavailable"]) {
        "upstream_unavailable"
    } else if text_has_any(
        text,
        &[
            "internal_server_error",
            "server_error",
            "an error occurred while processing your request",
        ],
    ) || (text.contains("you can retry your request") && text.contains("request id"))
    {
        "upstream_server_error"
    } else if status == StatusCode::FORBIDDEN
        && text_has_any(
            text,
            &[
                "cf-mitigated",
                "cf-chl-bypass",
                "_cf_chl",
                "cf_chl",
                "attention required",
                "just a moment",
            ],
        )
    {
        "upstream_edge_challenge"
    } else if status == StatusCode::FORBIDDEN {
        "upstream_forbidden"
    } else if status == StatusCode::NOT_FOUND {
        "upstream_not_found"
    } else if status == StatusCode::REQUEST_TIMEOUT {
        "upstream_request_timeout"
    } else if status == StatusCode::CONFLICT {
        "upstream_conflict"
    } else if status == StatusCode::UNPROCESSABLE_ENTITY {
        "upstream_invalid_request"
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        "upstream_rate_limited"
    } else if status == StatusCode::INTERNAL_SERVER_ERROR {
        "upstream_server_error"
    } else if status == StatusCode::BAD_GATEWAY {
        "upstream_bad_gateway"
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        "upstream_unavailable"
    } else if status == StatusCode::GATEWAY_TIMEOUT {
        "upstream_gateway_timeout"
    } else if status == StatusCode::BAD_REQUEST {
        "upstream_invalid_request"
    } else if status.is_server_error() {
        "upstream_server_error"
    } else {
        "upstream_status"
    };
    UpstreamErrorClassification {
        category,
        message: upstream_failure_message(category),
    }
}

fn upstream_failure_message(category: &str) -> &'static str {
    match category {
        "upstream_previous_response_not_found" => "previous response is unavailable",
        "upstream_tool_call_mismatch" => "tool output does not match an active tool call",
        "upstream_context_too_large" => "request context exceeds the model limit",
        "upstream_encrypted_content_invalid" => "encrypted reasoning context is invalid",
        "upstream_instructions_required" => "upstream requires request instructions",
        "upstream_usage_not_included" => "upstream account plan does not include this capability",
        "upstream_quota_exhausted" => "upstream usage quota is exhausted",
        "upstream_account_disabled" => "upstream account is disabled",
        "upstream_unauthorized" => "upstream authentication failed",
        "upstream_region_unsupported" => "upstream rejected the request region",
        "upstream_content_policy" => "upstream content policy rejected the request",
        "upstream_payload_too_large" => "upstream rejected the request size",
        "upstream_unsupported_request" => "upstream does not support this request",
        "upstream_model_not_found" => "upstream model is unavailable",
        "upstream_model_unsupported" => "upstream does not support this model",
        "upstream_model_capacity" => "upstream model is at capacity",
        "upstream_websocket_unsupported" => "upstream does not support WebSocket requests",
        "upstream_websocket_connection_limit" => "upstream WebSocket connection limit was reached",
        "upstream_rate_limited" => "upstream rate limit was reached",
        "upstream_edge_challenge" => "upstream edge security challenged the request",
        "upstream_forbidden" => "upstream access was forbidden",
        "upstream_not_found" => "upstream resource was not found",
        "upstream_request_timeout" => "upstream request timed out",
        "upstream_conflict" => "upstream request conflicted with current state",
        "upstream_invalid_request" => "upstream rejected the request",
        "upstream_overloaded" => "upstream service is overloaded",
        "upstream_server_error" => "upstream service failed",
        "upstream_bad_gateway" => "upstream gateway failed",
        "upstream_unavailable" => "upstream service is unavailable",
        "upstream_gateway_timeout" => "upstream gateway timed out",
        _ => "all eligible upstream sources failed",
    }
}

fn upstream_failure_status(category: &str) -> StatusCode {
    match category {
        "upstream_unauthorized" => StatusCode::UNAUTHORIZED,
        "upstream_account_disabled" | "upstream_forbidden" | "upstream_region_unsupported" => {
            StatusCode::FORBIDDEN
        }
        "upstream_usage_not_included" => StatusCode::FORBIDDEN,
        "upstream_quota_exhausted"
        | "upstream_rate_limited"
        | "upstream_websocket_connection_limit" => StatusCode::TOO_MANY_REQUESTS,
        "upstream_model_not_found" | "upstream_not_found" => StatusCode::NOT_FOUND,
        "upstream_model_unsupported" => StatusCode::NOT_ACCEPTABLE,
        "upstream_websocket_unsupported" => StatusCode::UPGRADE_REQUIRED,
        "upstream_request_timeout" => StatusCode::REQUEST_TIMEOUT,
        "upstream_conflict" => StatusCode::CONFLICT,
        "upstream_payload_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        "upstream_previous_response_not_found"
        | "upstream_tool_call_mismatch"
        | "upstream_context_too_large"
        | "upstream_encrypted_content_invalid"
        | "upstream_instructions_required"
        | "upstream_content_policy"
        | "upstream_unsupported_request"
        | "upstream_invalid_request" => StatusCode::BAD_REQUEST,
        "upstream_model_capacity"
        | "upstream_overloaded"
        | "upstream_unavailable"
        | "upstream_edge_challenge" => StatusCode::SERVICE_UNAVAILABLE,
        "upstream_server_error" => StatusCode::INTERNAL_SERVER_ERROR,
        "upstream_gateway_timeout" => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::BAD_GATEWAY,
    }
}

fn canonical_upstream_status(status: StatusCode, category: &str) -> StatusCode {
    if category == "upstream_status" {
        status
    } else {
        upstream_failure_status(category)
    }
}

fn upstream_error_text(value: &Value) -> String {
    const PATHS: &[&str] = &[
        "/code",
        "/type",
        "/message",
        "/msg",
        "/err",
        "/error_msg",
        "/detail",
        "/error_code",
        "/error",
        "/error/code",
        "/error/type",
        "/error/message",
        "/error/detail",
        "/body/code",
        "/body/type",
        "/body/message",
        "/body/error",
        "/body/error/code",
        "/body/error/type",
        "/body/error/message",
        "/response/code",
        "/response/type",
        "/response/message",
        "/response/error",
        "/response/error/code",
        "/response/error/type",
        "/response/error/message",
        "/response/incomplete_details/reason",
        "/header/message",
    ];
    let mut text = String::new();
    for value in PATHS
        .iter()
        .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    {
        if !text.is_empty() {
            text.push(' ');
        }
        text.extend(
            value
                .chars()
                .take(4_096)
                .map(|character| character.to_ascii_lowercase()),
        );
    }
    text
}

fn normalized_error_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(4_096)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn text_has_any(text: &str, values: &[&str]) -> bool {
    values.iter().any(|value| text.contains(value))
}

fn upstream_status_from_value(value: &Value) -> Option<StatusCode> {
    [
        "/status",
        "/status_code",
        "/error/status",
        "/error/status_code",
        "/body/status",
        "/body/status_code",
        "/body/error/status",
        "/body/error/status_code",
        "/response/status",
        "/response/status_code",
        "/response/error/status",
        "/response/error/status_code",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path))
    .find_map(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|status| status.trim().parse().ok()))
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| *status > 0)
            .and_then(|status| StatusCode::from_u16(status).ok())
    })
}

fn upstream_event_failure_category(
    event_type: Option<&str>,
    value: &Value,
) -> Option<&'static str> {
    match event_type {
        Some("response.incomplete") => Some("response_incomplete"),
        Some("response.cancelled" | "response.canceled") => Some("upstream_cancelled"),
        Some("response.failed" | "error") => {
            let classification = classify_upstream_error_value(
                upstream_status_from_value(value).unwrap_or(StatusCode::BAD_GATEWAY),
                value,
            );
            Some(
                if classification.category == "upstream_bad_gateway"
                    && upstream_status_from_value(value).is_none()
                {
                    "upstream_terminal"
                } else {
                    classification.category
                },
            )
        }
        _ => None,
    }
}

impl AttemptFailure {
    fn transport(error: &reqwest::Error) -> Self {
        let (category, message) = if error.is_timeout() {
            ("upstream_transport_timeout", "upstream request timed out")
        } else if error.is_connect() {
            (
                "upstream_transport_connect",
                "upstream connection could not be established",
            )
        } else if error.is_body() {
            (
                "upstream_transport_body",
                "upstream request or response body failed",
            )
        } else if error.is_request() {
            ("upstream_transport_request", "upstream request failed")
        } else {
            ("upstream_transport", "upstream transport failed")
        };
        Self {
            status: StatusCode::BAD_GATEWAY,
            category,
            message,
            cooldown_ms: TRANSPORT_COOLDOWN_MS,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    fn body() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_error",
            message: "upstream response failed",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    fn invalid_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message: "request cannot be translated for an eligible source",
            cooldown_ms: 0,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    fn translation() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_translation",
            message: "upstream response could not be translated",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    fn status_with_body(status: StatusCode, body: Option<&[u8]>) -> Self {
        let classification = classify_upstream_error(status, body);
        Self {
            status: canonical_upstream_status(status, classification.category),
            category: classification.category,
            message: classification.message,
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            cooldown_hint: body.map(rate_limit_body_hint).unwrap_or_default(),
        }
    }

    fn classified_with_hint(
        status: StatusCode,
        category: &'static str,
        cooldown_hint: RateLimitBodyHint,
    ) -> Self {
        Self {
            status: canonical_upstream_status(status, category),
            category,
            message: upstream_failure_message(category),
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            cooldown_hint,
        }
    }

    fn stream(category: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category,
            message: "upstream stream failed before the first event",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    fn no_candidate() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible source is available for this model",
            cooldown_ms: 0,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    fn prepare(error: ExecutorPrepareError) -> Self {
        match error {
            ExecutorPrepareError::Authentication | ExecutorPrepareError::InvalidCredential => {
                Self {
                    status: StatusCode::UNAUTHORIZED,
                    category: "account_auth",
                    message: "account authorization is unavailable",
                    cooldown_ms: 30 * 60_000,
                    cooldown_hint: RateLimitBodyHint::default(),
                }
            }
            ExecutorPrepareError::Persistence => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                category: "account_token_persistence",
                message: "refreshed account authorization could not be persisted",
                cooldown_ms: TRANSIENT_COOLDOWN_MS,
                cooldown_hint: RateLimitBodyHint::default(),
            },
            ExecutorPrepareError::Transient => Self {
                status: StatusCode::BAD_GATEWAY,
                category: "account_refresh",
                message: "account authorization refresh failed",
                cooldown_ms: TRANSIENT_COOLDOWN_MS,
                cooldown_hint: RateLimitBodyHint::default(),
            },
        }
    }
}

fn retryable_status(status: StatusCode, has_previous_response_id: bool) -> bool {
    matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::FORBIDDEN
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::CONFLICT
            | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
        || (status == StatusCode::NOT_FOUND && !has_previous_response_id)
}

fn retryable_failure(status: StatusCode, category: &str, has_previous_response_id: bool) -> bool {
    if !failure_category_requires_cooldown(category) {
        return false;
    }
    retryable_status(status, has_previous_response_id)
        || matches!(
            category,
            "upstream_unauthorized"
                | "upstream_account_disabled"
                | "upstream_usage_not_included"
                | "upstream_quota_exhausted"
                | "upstream_region_unsupported"
                | "upstream_model_not_found"
                | "upstream_model_unsupported"
                | "upstream_model_capacity"
                | "upstream_websocket_connection_limit"
                | "upstream_rate_limited"
                | "upstream_request_timeout"
                | "upstream_overloaded"
                | "upstream_edge_challenge"
                | "upstream_server_error"
                | "upstream_bad_gateway"
                | "upstream_unavailable"
                | "upstream_gateway_timeout"
        )
}

fn failure_category_requires_cooldown(category: &str) -> bool {
    !matches!(
        category,
        "client_cancelled"
            | "response_affinity_miss"
            | "response_incomplete"
            | "upstream_cancelled"
            | "upstream_previous_response_not_found"
            | "upstream_tool_call_mismatch"
            | "upstream_context_too_large"
            | "upstream_encrypted_content_invalid"
            | "upstream_instructions_required"
            | "upstream_content_policy"
            | "upstream_payload_too_large"
            | "upstream_unsupported_request"
            | "upstream_websocket_unsupported"
            | "upstream_invalid_request"
    )
}

fn failure_category_is_request_terminal(category: &str) -> bool {
    matches!(
        category,
        "upstream_tool_call_mismatch"
            | "upstream_context_too_large"
            | "upstream_encrypted_content_invalid"
            | "upstream_instructions_required"
            | "upstream_content_policy"
            | "upstream_payload_too_large"
            | "upstream_unsupported_request"
            | "upstream_websocket_unsupported"
            | "upstream_invalid_request"
    )
}

fn recoverable_response_affinity_miss(
    status: StatusCode,
    has_previous_response_id: bool,
    _response_affinity_hit: bool,
    previous_response_not_found: bool,
) -> bool {
    has_previous_response_id
        && previous_response_not_found
        && matches!(status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND)
}

fn retry_candidate_limit(max_retry_candidates: usize, owner_recovery_confirmed: bool) -> usize {
    if owner_recovery_confirmed {
        MAX_RESPONSE_OWNER_CANDIDATES
    } else {
        max_retry_candidates
    }
}

fn previous_response_not_found(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .ok()
        .is_some_and(|value| previous_response_not_found_value(&value))
}

fn previous_response_not_found_value(value: &Value) -> bool {
    [value.pointer("/error/code"), value.pointer("/error/type")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("previous_response_not_found")
        })
        || [value.pointer("/error/message"), value.get("message")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(previous_response_not_found_message)
}

fn previous_response_not_found_message(message: &str) -> bool {
    let message = message.trim().trim_end_matches('.').to_ascii_lowercase();
    message == "previous response not found"
        || (message.starts_with("previous response with id ") && message.ends_with(" not found"))
        || message.starts_with("no response found for previous_response_id ")
}

pub(super) fn contains_function_call_output(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_function_call_output),
        Value::Object(object) => {
            object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "function_call_output")
                || object.values().any(contains_function_call_output)
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_failure_cooldown_with_body(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    status: StatusCode,
    category: &str,
    headers: &reqwest::header::HeaderMap,
    body: Option<&[u8]>,
    half_open_probe: bool,
) -> FailureState {
    let hint = body.map(rate_limit_body_hint).unwrap_or_default();
    apply_failure_cooldown_with_hint(
        runtime,
        candidate_id,
        model,
        status,
        category,
        headers,
        hint,
        half_open_probe,
    )
}

fn apply_attempt_failure_cooldown(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    failure: &AttemptFailure,
    headers: &reqwest::header::HeaderMap,
    half_open_probe: bool,
) -> FailureState {
    apply_failure_cooldown_with_hint(
        runtime,
        candidate_id,
        model,
        failure.status,
        failure.category,
        headers,
        failure.cooldown_hint,
        half_open_probe,
    )
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RateLimitBodyHint {
    retry_after_ms: Option<u64>,
    global: bool,
}

fn apply_status_cooldown_with_hint(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    hint: RateLimitBodyHint,
    half_open_probe: bool,
) -> FailureState {
    let consecutive_failures = runtime.record_failure(candidate_id);
    let now_system = SystemTime::now();
    let now = now_system
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let (scope, duration_ms) = match status {
        StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN => {
            ("*", 30 * 60_000)
        }
        StatusCode::NOT_FOUND => (model, TRANSIENT_COOLDOWN_MS),
        StatusCode::TOO_MANY_REQUESTS => {
            let duration_ms = rate_limit_cooldown_ms(
                retry_after_ms(headers, now_system),
                hint.retry_after_ms,
                consecutive_failures,
            );
            (if hint.global { "*" } else { model }, duration_ms)
        }
        _ => ("*", TRANSIENT_COOLDOWN_MS),
    };
    let duration_ms = half_open_backoff_ms(duration_ms, consecutive_failures, half_open_probe);
    let retry_at_ms = now.saturating_add(duration_ms);
    runtime.set_cooldown(candidate_id, scope, retry_at_ms);
    FailureState {
        cooldown_scope: scope.to_string(),
        retry_at_ms,
        consecutive_failures,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_failure_cooldown_with_hint(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    status: StatusCode,
    category: &str,
    headers: &reqwest::header::HeaderMap,
    hint: RateLimitBodyHint,
    half_open_probe: bool,
) -> FailureState {
    let status = canonical_upstream_status(status, category);
    if matches!(
        category,
        "upstream_model_not_found"
            | "upstream_model_unsupported"
            | "upstream_model_capacity"
            | "upstream_overloaded"
    ) {
        return apply_cooldown(
            runtime,
            candidate_id,
            model,
            TRANSIENT_COOLDOWN_MS,
            half_open_probe,
        );
    }
    apply_status_cooldown_with_hint(
        runtime,
        candidate_id,
        model,
        status,
        headers,
        hint,
        half_open_probe,
    )
}

fn rate_limit_body_hint(body: &[u8]) -> RateLimitBodyHint {
    rate_limit_body_hint_at(body, SystemTime::now())
}

fn rate_limit_body_hint_at(body: &[u8], now: SystemTime) -> RateLimitBodyHint {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return RateLimitBodyHint::default();
    };
    rate_limit_body_hint_value(&value, now)
}

fn rate_limit_body_hint_value(value: &Value, now: SystemTime) -> RateLimitBodyHint {
    let retry_after_ms = rate_limit_reset_delay_ms(value, now)
        .or_else(|| {
            [
                "/resets_in_seconds",
                "/error/resets_in_seconds",
                "/body/error/resets_in_seconds",
                "/response/error/resets_in_seconds",
            ]
            .into_iter()
            .find_map(|path| value.pointer(path).and_then(json_seconds_to_ms))
        })
        .or_else(|| {
            [
                "/retry_after",
                "/error/retry_after",
                "/body/error/retry_after",
                "/response/error/retry_after",
            ]
            .into_iter()
            .find_map(|path| value.pointer(path).and_then(json_seconds_to_ms))
        })
        .or_else(|| retry_delay_from_text(&upstream_error_text(value)));
    let global = [
        "/type",
        "/code",
        "/error/type",
        "/error/code",
        "/body/error/type",
        "/body/error/code",
        "/response/error/type",
        "/response/error/code",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .map(str::to_ascii_lowercase)
    .any(|kind| {
        kind.contains("usage_limit")
            || kind.contains("usage_not_included")
            || kind.contains("quota")
            || kind.contains("credits_depleted")
            || matches!(
                kind.as_str(),
                "rate_limit_reached" | "websocket_connection_limit_reached"
            )
    });
    RateLimitBodyHint {
        retry_after_ms,
        global,
    }
}

fn rate_limit_reset_delay_ms(value: &Value, now: SystemTime) -> Option<u64> {
    let reset_at = [
        "/resets_at",
        "/error/resets_at",
        "/body/error/resets_at",
        "/response/error/resets_at",
    ]
    .into_iter()
    .find_map(|path| value.pointer(path).and_then(json_u64))?;
    let reset_seconds = if reset_at > 10_000_000_000 {
        reset_at / 1_000
    } else {
        reset_at
    };
    let now_seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    reset_seconds
        .checked_sub(now_seconds)
        .and_then(|seconds| seconds.checked_mul(1_000))
        .filter(|duration_ms| *duration_ms > 0)
        .map(|duration_ms| duration_ms.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

fn json_seconds_to_ms(value: &Value) -> Option<u64> {
    let seconds = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Some(
        (seconds * 1_000.0)
            .ceil()
            .min(MAX_RATE_LIMIT_RETRY_HINT_MS as f64) as u64,
    )
}

fn retry_delay_from_text(text: &str) -> Option<u64> {
    let suffix = text.split_once("try again in")?.1.trim_start();
    let number_end = suffix
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(suffix.len());
    let seconds_or_millis = suffix[..number_end].parse::<f64>().ok()?;
    if !seconds_or_millis.is_finite() || seconds_or_millis <= 0.0 {
        return None;
    }
    let unit = suffix[number_end..].trim_start();
    let multiplier = if unit.starts_with("ms") || unit.starts_with("millisecond") {
        1.0
    } else if unit.starts_with('s') || unit.starts_with("second") {
        1_000.0
    } else {
        return None;
    };
    Some(
        (seconds_or_millis * multiplier)
            .ceil()
            .min(MAX_RATE_LIMIT_RETRY_HINT_MS as f64) as u64,
    )
}

fn rate_limit_cooldown_ms(
    header_delay_ms: Option<u64>,
    body_delay_ms: Option<u64>,
    consecutive_failures: u32,
) -> u64 {
    match (header_delay_ms, body_delay_ms) {
        (Some(header), Some(body)) => header.max(body),
        (Some(header), None) => header,
        (None, Some(body)) => body,
        (None, None) => exponential_backoff_ms(consecutive_failures),
    }
}

fn apply_cooldown(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    scope: &str,
    duration_ms: u64,
    half_open_probe: bool,
) -> FailureState {
    let consecutive_failures = runtime.record_failure(candidate_id);
    let duration_ms = half_open_backoff_ms(duration_ms, consecutive_failures, half_open_probe);
    let retry_at_ms = now_ms().saturating_add(duration_ms);
    runtime.set_cooldown(candidate_id, scope, retry_at_ms);
    FailureState {
        cooldown_scope: scope.to_string(),
        retry_at_ms,
        consecutive_failures,
    }
}

fn apply_failure_state(event: &mut UsageEvent, state: FailureState) {
    event.cooldown_scope = Some(state.cooldown_scope);
    event.retry_at_ms = Some(state.retry_at_ms);
    event.consecutive_failures = Some(state.consecutive_failures);
}

fn retry_after_ms(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<u64> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    let duration_ms = if let Ok(seconds) = value.parse::<u64>() {
        seconds.saturating_mul(1_000)
    } else {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .ok()?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    };
    Some(duration_ms.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

fn exponential_backoff_ms(consecutive_failures: u32) -> u64 {
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    1_000_u64
        .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(MAX_RATE_LIMIT_COOLDOWN_MS)
}

fn half_open_backoff_ms(duration_ms: u64, consecutive_failures: u32, half_open_probe: bool) -> u64 {
    if !half_open_probe {
        return duration_ms;
    }
    let duration_ms = duration_ms.max(1_000);
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    duration_ms
        .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(duration_ms.max(MAX_RATE_LIMIT_COOLDOWN_MS))
}

fn candidate_protocols(wire_api: WireApi) -> &'static [WireApi] {
    match wire_api {
        WireApi::Responses => &[WireApi::Responses, WireApi::ChatCompletions],
        WireApi::ChatCompletions => &[WireApi::ChatCompletions, WireApi::Responses],
        WireApi::Messages => &[WireApi::Messages],
    }
}

fn valid_local_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
    else {
        return false;
    };
    host.host().eq_ignore_ascii_case("localhost")
        || host
            .host()
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn invalid_host() -> Response<Body> {
    api_error(
        StatusCode::MISDIRECTED_REQUEST,
        "Host must target the local gateway",
        "invalid_host",
    )
}

fn unauthorized() -> Response<Body> {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "local API key is missing or invalid",
        "invalid_api_key",
    );
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn cooldown_error(retry_at_ms: u64) -> Response<Body> {
    let seconds = retry_at_ms
        .saturating_sub(now_ms())
        .saturating_add(999)
        .checked_div(1_000)
        .unwrap_or_default()
        .max(1);
    let mut response = api_error(
        StatusCode::TOO_MANY_REQUESTS,
        "all eligible sources are cooling down",
        "all_sources_cooling_down",
    );
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

fn api_error(status: StatusCode, message: &str, code: &str) -> Response<Body> {
    let error_type = api_error_type(status);
    let code = api_error_code(code);
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": code,
            }
        })),
    )
        .into_response()
}

fn api_error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        status if status.is_server_error() => "server_error",
        _ => "invalid_request_error",
    }
}

fn api_error_code(code: &str) -> &str {
    match code {
        "upstream_unauthorized" => "invalid_api_key",
        "upstream_account_disabled" => "account_deactivated",
        "upstream_usage_not_included" => "usage_not_included",
        "upstream_quota_exhausted" => "insufficient_quota",
        "upstream_rate_limited" => "rate_limit_exceeded",
        "upstream_context_too_large" => "context_too_large",
        "upstream_encrypted_content_invalid" => "invalid_encrypted_content",
        "upstream_instructions_required" => "missing_required_parameter",
        "upstream_previous_response_not_found" => "previous_response_not_found",
        "upstream_tool_call_mismatch" => "tool_call_not_found",
        "upstream_content_policy" => "content_policy_violation",
        "upstream_payload_too_large" => "request_too_large",
        "upstream_unsupported_request" => "unsupported_request",
        "upstream_model_not_found" => "model_not_found",
        "upstream_model_unsupported" => "model_not_supported",
        "upstream_model_capacity" => "model_at_capacity",
        "upstream_websocket_unsupported" => "websocket_not_supported",
        "upstream_websocket_connection_limit" => "websocket_connection_limit_reached",
        "upstream_region_unsupported" => "unsupported_country_region_territory",
        "upstream_edge_challenge" => "edge_security_challenge",
        "upstream_forbidden" => "permission_denied",
        "upstream_not_found" => "not_found",
        "upstream_request_timeout" => "request_timeout",
        "upstream_conflict" => "conflict",
        "upstream_invalid_request" => "invalid_request",
        "upstream_overloaded" => "server_is_overloaded",
        "upstream_server_error" => "internal_server_error",
        "upstream_bad_gateway" => "bad_gateway",
        "upstream_unavailable" => "service_unavailable",
        "upstream_gateway_timeout" => "gateway_timeout",
        _ => code,
    }
}

fn proxy_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    for name in [CONTENT_TYPE, CACHE_CONTROL] {
        if let Some(value) = upstream_headers.get(name.as_str()) {
            response.headers_mut().insert(name, value.clone());
        }
    }
    response
}

fn proxy_sse_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    if let Some(value) = upstream_headers.get(CACHE_CONTROL.as_str()) {
        response.headers_mut().insert(CACHE_CONTROL, value.clone());
    }
    response
}

fn proxy_json_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(value) = upstream_headers.get(CACHE_CONTROL.as_str()) {
        response.headers_mut().insert(CACHE_CONTROL, value.clone());
    }
    response
}

#[allow(clippy::too_many_arguments)]
fn usage_event(
    request_id: &str,
    attempt: u16,
    local_key_id: &str,
    route: &ExecutorRoute,
    requested_model: &str,
    success: bool,
    http_status: u16,
    error_category: Option<String>,
    latency_ms: u64,
) -> UsageEvent {
    UsageEvent {
        request_id: request_id.to_string(),
        attempt,
        local_key_id: local_key_id.to_string(),
        source_id: route.source_id.clone(),
        candidate_id: Some(route.candidate_id.clone()),
        account_id: route.account_id.clone(),
        routing: route.routing.clone(),
        requested_model: Some(requested_model.to_string()),
        resolved_model: Some(route.source_model.clone()),
        wire_api: route.wire_api,
        success,
        http_status,
        error_category,
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
    }
}

fn populate_tokens(event: &mut UsageEvent, body: &[u8]) {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let Some(usage) = find_usage(&body) else {
        return;
    };
    apply_usage(event, usage);
}

fn emit_usage(runtime: &GatewayRuntime, event: UsageEvent) {
    emit_callback(&runtime.usage, event);
}

fn emit_callback(callback: &crate::UsageCallback, event: UsageEvent) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)));
}

fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("relay-{timestamp}-{sequence}")
}

struct UsageStream<S> {
    inner: Pin<Box<S>>,
    callback: crate::UsageCallback,
    completion: CompletionCallback,
    event: Option<UsageEvent>,
    response_id: Option<String>,
    cooldown_hint: RateLimitBodyHint,
    started: Instant,
    sse_pending: Vec<u8>,
    output_pending: VecDeque<Bytes>,
    terminated: bool,
}

impl<S> UsageStream<S> {
    fn new(
        stream: S,
        callback: crate::UsageCallback,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            callback,
            completion,
            event: Some(event),
            response_id: None,
            cooldown_hint: RateLimitBodyHint::default(),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            terminated: false,
        }
    }

    fn finish(&mut self, success: Option<bool>, category: Option<&str>) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        if let Some(success) = success {
            event.success = success;
        }
        if let Some(category) = category {
            event.error_category = Some(category.to_string());
        }
        if !event.success && event.http_status < 400 {
            event.http_status = event
                .error_category
                .as_deref()
                .filter(|category| *category != "client_cancelled")
                .map(upstream_failure_status)
                .unwrap_or(StatusCode::BAD_GATEWAY)
                .as_u16();
        }
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        event.generation_ms = event
            .ttft_ms
            .map(|ttft_ms| event.latency_ms.saturating_sub(ttft_ms))
            .filter(|duration| *duration > 0);
        (self.completion)(&mut event, self.response_id.as_deref(), self.cooldown_hint);
        emit_callback(&self.callback, event);
    }

    fn queue_responses_failure(&mut self, category: &str) -> bool {
        let Some(event) = self.event.as_ref() else {
            return false;
        };
        if event.wire_api != WireApi::Responses {
            return false;
        }
        let response_id = self.response_id.clone().unwrap_or_else(|| {
            let suffix = event
                .request_id
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>();
            format!("resp_{suffix}")
        });
        let message = match category {
            "stream_invalid" => "Upstream returned an invalid streaming event",
            "stream_event_too_large" => "Upstream streaming event exceeded the size limit",
            "stream_incomplete" => "Upstream stream ended before response.completed",
            _ => "Upstream stream disconnected before completion",
        };
        let payload = json!({
            "type": "response.failed",
            "response": {
                "id": response_id,
                "object": "response",
                "model": event.requested_model.clone().unwrap_or_default(),
                "status": "failed",
                "output": [],
                "error": {
                    "type": "stream_error",
                    "code": category,
                    "message": message,
                }
            }
        });
        let Ok(payload) = serde_json::to_vec(&payload) else {
            return false;
        };
        let mut frame = Vec::with_capacity(payload.len() + 44);
        frame.extend_from_slice(b"event: response.failed\ndata: ");
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(b"\n\n");
        self.output_pending.push_back(Bytes::from(frame));
        true
    }

    fn fail_stream(&mut self, category: &str) -> bool {
        let framed = self.queue_responses_failure(category);
        self.finish(Some(false), Some(category));
        self.terminated = true;
        framed
    }

    fn ingest_sse(&mut self, bytes: &[u8]) {
        if self.terminated {
            return;
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream("stream_event_too_large");
            return;
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.sse_pending) {
            if end > MAX_SSE_EVENT_BYTES {
                self.sse_pending.clear();
                self.fail_stream("stream_event_too_large");
                return;
            }
            let event = self.sse_pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                self.sse_pending.clear();
                self.fail_stream("stream_invalid");
                return;
            }
            if terminal.has_output_delta
                && self
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none())
            {
                if let Some(current) = self.event.as_mut() {
                    current.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
                }
            }
            if let Some(usage) = terminal.usage {
                if let Some(current) = self.event.as_mut() {
                    apply_usage(current, &usage);
                }
            }
            if terminal.response_id.is_some() {
                self.response_id = terminal.response_id;
            }
            self.output_pending.push_back(Bytes::from(event));
            match terminal.outcome {
                Some(TerminalOutcome::Success) => {
                    self.finish(None, None);
                    self.terminated = true;
                    return;
                }
                Some(TerminalOutcome::Failure) => {
                    self.cooldown_hint = terminal.cooldown_hint;
                    self.finish(
                        Some(false),
                        Some(terminal.error_category.unwrap_or("upstream_terminal")),
                    );
                    self.terminated = true;
                    return;
                }
                None => {}
            }
        }
        if self.sse_pending.len() > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.fail_stream("stream_event_too_large");
        }
    }
}

impl<S, E> Stream for UsageStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, E>>,
{
    type Item = std::result::Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if let Some(bytes) = this.output_pending.pop_front() {
                return Poll::Ready(Some(Ok(bytes)));
            }
            if this.terminated {
                return Poll::Ready(None);
            }
            match this.inner.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(bytes))) => this.ingest_sse(&bytes),
                Poll::Ready(Some(Err(error))) => {
                    if this.fail_stream("upstream_stream") {
                        continue;
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if this.event.as_ref().is_some_and(|event| event.success) {
                        if this.fail_stream("stream_incomplete") {
                            continue;
                        }
                    } else {
                        this.finish(None, None);
                    }
                    this.sse_pending.clear();
                    this.terminated = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[derive(Default)]
struct TerminalEvent {
    has_data: bool,
    valid: bool,
    has_output_delta: bool,
    outcome: Option<TerminalOutcome>,
    error_status: Option<StatusCode>,
    error_category: Option<&'static str>,
    cooldown_hint: RateLimitBodyHint,
    usage: Option<Value>,
    response_id: Option<String>,
    response: Option<Value>,
    output_item: Option<Value>,
}

#[derive(Debug, Eq, PartialEq)]
enum TerminalOutcome {
    Success,
    Failure,
}

fn sse_event_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| position + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
        })
}

fn parse_sse_event(event: &[u8]) -> TerminalEvent {
    let mut data = Vec::new();
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(value) = line.strip_prefix(b"data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
    }
    if data.is_empty() {
        return TerminalEvent::default();
    }
    if data == b"[DONE]" {
        return TerminalEvent {
            has_data: true,
            valid: true,
            has_output_delta: false,
            outcome: Some(TerminalOutcome::Success),
            error_status: None,
            error_category: None,
            cooldown_hint: RateLimitBodyHint::default(),
            usage: None,
            response_id: None,
            response: None,
            output_item: None,
        };
    }
    let Ok(value) = serde_json::from_slice::<Value>(&data) else {
        return TerminalEvent {
            has_data: true,
            ..TerminalEvent::default()
        };
    };
    let event_type = value.get("type").and_then(Value::as_str);
    let outcome = match event_type {
        Some("response.completed" | "response.done" | "message_stop") => {
            Some(TerminalOutcome::Success)
        }
        Some(
            "response.failed"
            | "response.incomplete"
            | "response.cancelled"
            | "response.canceled"
            | "error",
        ) => Some(TerminalOutcome::Failure),
        _ => None,
    };
    let error_category = upstream_event_failure_category(event_type, &value);
    let error_status = error_category.map(|category| {
        let status = upstream_status_from_value(&value)
            .filter(|status| !status.is_success())
            .unwrap_or_else(|| upstream_failure_status(category));
        canonical_upstream_status(status, category)
    });
    let cooldown_hint = rate_limit_body_hint_value(&value, SystemTime::now());
    let has_output_delta = has_output_delta(&value, event_type);
    let usage = find_usage(&value).cloned();
    let response_id = response_id(&value).map(str::to_string);
    let response = value.get("response").cloned();
    let output_item = (value.get("type").and_then(Value::as_str)
        == Some("response.output_item.done"))
    .then(|| value.get("item").cloned())
    .flatten();
    TerminalEvent {
        has_data: true,
        valid: true,
        has_output_delta,
        outcome,
        error_status,
        error_category,
        cooldown_hint,
        usage,
        response_id,
        response,
        output_item,
    }
}

fn has_output_delta(value: &Value, event_type: Option<&str>) -> bool {
    if matches!(
        event_type,
        Some(
            "response.output_text.delta"
                | "response.refusal.delta"
                | "response.function_call_arguments.delta"
                | "response.custom_tool_call_input.delta"
                | "response.mcp_call_arguments.delta"
                | "response.code_interpreter_call_code.delta"
        )
    ) && value
        .get("delta")
        .and_then(Value::as_str)
        .is_some_and(|delta| !delta.is_empty())
    {
        return true;
    }
    if event_type == Some("content_block_delta")
        && value.get("delta").is_some_and(|delta| {
            ["text", "partial_json"].into_iter().any(|key| {
                delta
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            })
        })
    {
        return true;
    }
    value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.iter().any(chat_choice_has_output_delta))
}

fn chat_choice_has_output_delta(choice: &Value) -> bool {
    let Some(delta) = choice.get("delta") else {
        return false;
    };
    ["content", "refusal"].into_iter().any(|key| {
        delta
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    }) || delta
        .get("function_call")
        .is_some_and(function_delta_has_output)
        || delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
                        || call.get("function").is_some_and(function_delta_has_output)
                })
            })
}

fn function_delta_has_output(function: &Value) -> bool {
    ["name", "arguments"].into_iter().any(|key| {
        function
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    })
}

fn completed_account_response(bytes: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    if serde_json::from_slice::<Value>(bytes).is_ok() {
        return Ok(bytes.to_vec());
    }
    let mut offset = 0;
    let mut output = Vec::new();
    while let Some(end) = sse_event_end(&bytes[offset..]) {
        let terminal = parse_sse_event(&bytes[offset..offset + end]);
        if terminal.has_data && !terminal.valid {
            return Err(AttemptFailure::stream("stream_invalid"));
        }
        if let Some(item) = terminal.output_item {
            output.push(item);
        }
        match terminal.outcome {
            Some(TerminalOutcome::Failure) => {
                let category = terminal.error_category.unwrap_or("upstream_terminal");
                return Err(AttemptFailure::classified_with_hint(
                    terminal
                        .error_status
                        .unwrap_or_else(|| upstream_failure_status(category)),
                    category,
                    terminal.cooldown_hint,
                ));
            }
            Some(TerminalOutcome::Success) => {
                if let Some(mut response) = terminal.response {
                    if response
                        .get("output")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
                    {
                        response["output"] = Value::Array(output);
                    }
                    return serde_json::to_vec(&response)
                        .map_err(|_| AttemptFailure::stream("stream_invalid"));
                }
            }
            None => {}
        }
        offset += end;
    }
    Err(AttemptFailure::stream("stream_incomplete"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn apply_usage(event: &mut UsageEvent, usage: &Value) {
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64);
    event.input_tokens = input_tokens;
    event.cached_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
        })
        .or_else(|| usage.get("cached_tokens"))
        .and_then(Value::as_u64)
        .map(|cached| cached.min(input_tokens.unwrap_or(cached)));
    event.cache_write_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cache_write_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cache_write_tokens"))
        })
        .or_else(|| usage.get("cache_write_tokens"))
        .and_then(Value::as_u64)
        .map(|written| {
            written.min(
                input_tokens
                    .unwrap_or(written)
                    .saturating_sub(event.cached_input_tokens.unwrap_or_default()),
            )
        });
    event.reasoning_tokens = usage
        .get("reasoning_tokens")
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .and_then(Value::as_u64)
        .map(|reasoning| reasoning.min(output_tokens.unwrap_or(reasoning)));
    event.output_tokens = output_tokens;
    let reported_total = usage.get("total_tokens").and_then(Value::as_u64);
    event.total_tokens = match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => {
            let measured = input.saturating_add(output);
            Some(reported_total.unwrap_or(measured).max(measured))
        }
        _ => reported_total,
    };
}

fn find_usage(value: &Value) -> Option<&Value> {
    value.get("usage").or_else(|| {
        let response = value.get("response")?;
        response.get("usage").or_else(|| {
            response
                .get("response")
                .and_then(|nested| nested.get("usage"))
        })
    })
}

fn response_id(value: &Value) -> Option<&str> {
    value
        .pointer("/response/response/id")
        .or_else(|| value.pointer("/response/id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn response_id_from_bytes(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| response_id(&value).map(str::to_string))
}

impl<S> Drop for UsageStream<S> {
    fn drop(&mut self) {
        self.finish(Some(false), Some("client_cancelled"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{convert::Infallible, sync::Mutex, time::Duration};

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
    fn bad_request_affinity_recovery_requires_a_structured_missing_response_error() {
        for payload in [
            br#"{"error":{"code":"previous_response_not_found"}}"#.as_slice(),
            br#"{"message":"Previous response with id 'resp_123' not found."}"#.as_slice(),
        ] {
            assert!(recoverable_response_affinity_miss(
                StatusCode::BAD_REQUEST,
                true,
                false,
                previous_response_not_found(payload),
            ));
        }
        for payload in [
            br#"{"error":{"code":"invalid_request","message":"Invalid request body."}}"#.as_slice(),
            b"Previous response with id 'resp_123' not found.".as_slice(),
        ] {
            assert!(!recoverable_response_affinity_miss(
                StatusCode::BAD_REQUEST,
                true,
                false,
                previous_response_not_found(payload),
            ));
        }
        assert!(recoverable_response_affinity_miss(
            StatusCode::BAD_REQUEST,
            true,
            true,
            true,
        ));
        assert!(!recoverable_response_affinity_miss(
            StatusCode::BAD_REQUEST,
            true,
            true,
            false,
        ));
    }

    #[test]
    fn upstream_errors_use_stable_status_and_body_categories() {
        let cases = [
            (
                StatusCode::UNAUTHORIZED,
                br#"{"error":{"code":"invalid_api_key"}}"#.as_slice(),
                "upstream_unauthorized",
            ),
            (
                StatusCode::FORBIDDEN,
                br#"{"error":{"code":"account_deactivated"}}"#.as_slice(),
                "upstream_account_disabled",
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                br#"{"error":{"code":"deactivated_workspace"}}"#.as_slice(),
                "upstream_account_disabled",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"type":"usage_not_included"}}"#.as_slice(),
                "upstream_usage_not_included",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"type":"insufficient_quota"}}"#.as_slice(),
                "upstream_quota_exhausted",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":"rate_limit_exceeded"}}"#.as_slice(),
                "upstream_rate_limited",
            ),
            (
                StatusCode::NOT_FOUND,
                br#"{"error":{"code":"model_not_found"}}"#.as_slice(),
                "upstream_model_not_found",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"unsupported_parameter"}}"#.as_slice(),
                "upstream_unsupported_request",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"previous_response_not_found"}}"#.as_slice(),
                "upstream_previous_response_not_found",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"No tool call found for custom tool call output with call_id call_1"}}"#.as_slice(),
                "upstream_tool_call_mismatch",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"No tool output found for apply patch call call_1"}}"#.as_slice(),
                "upstream_tool_call_mismatch",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"context_length_exceeded"}}"#.as_slice(),
                "upstream_context_too_large",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"invalid_encrypted_content"}}"#.as_slice(),
                "upstream_encrypted_content_invalid",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"Instructions are required"}}"#.as_slice(),
                "upstream_instructions_required",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"response":{"error":{"code":"invalid_prompt"}}}"#.as_slice(),
                "upstream_invalid_request",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"response":{"error":{"code":"bio_policy"}}}"#.as_slice(),
                "upstream_content_policy",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"model_at_capacity"}}"#.as_slice(),
                "upstream_model_capacity",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"token_invalidated"}}"#.as_slice(),
                "upstream_unauthorized",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"An error occurred while processing your request"}}"#.as_slice(),
                "upstream_server_error",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":"server_is_overloaded"}}"#.as_slice(),
                "upstream_overloaded",
            ),
            (
                StatusCode::NOT_ACCEPTABLE,
                b"".as_slice(),
                "upstream_model_unsupported",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"invalid_request_error","message":"The 'gpt-next' model is not supported when using Codex with a ChatGPT account."}}"#.as_slice(),
                "upstream_model_unsupported",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"websocket_not_supported"}}"#.as_slice(),
                "upstream_websocket_unsupported",
            ),
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                b"Failed to buffer request body: length limit exceeded".as_slice(),
                "upstream_payload_too_large",
            ),
            (
                StatusCode::FORBIDDEN,
                b"<!doctype html><title>Just a moment...</title>".as_slice(),
                "upstream_edge_challenge",
            ),
            (StatusCode::CONFLICT, b"".as_slice(), "upstream_conflict"),
            (
                StatusCode::from_u16(529).unwrap(),
                b"server overloaded".as_slice(),
                "upstream_overloaded",
            ),
        ];
        for (status, body, expected) in cases {
            assert_eq!(
                classify_upstream_error(status, Some(body)).category,
                expected,
                "status={status} body={}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn retry_policy_matches_account_failover_and_official_transient_statuses() {
        assert!(retryable_status(StatusCode::UNAUTHORIZED, false));
        assert!(retryable_status(StatusCode::CONFLICT, false));
        assert!(retryable_status(StatusCode::from_u16(529).unwrap(), false));
        assert!(!retryable_status(StatusCode::PAYLOAD_TOO_LARGE, false));
        assert!(!retryable_status(StatusCode::BAD_REQUEST, false));
        assert!(retryable_failure(
            StatusCode::BAD_REQUEST,
            "upstream_model_capacity",
            false
        ));
        assert!(retryable_failure(
            StatusCode::BAD_REQUEST,
            "upstream_overloaded",
            false
        ));
        assert!(retryable_failure(
            StatusCode::BAD_GATEWAY,
            "upstream_usage_not_included",
            false
        ));
        assert!(!retryable_failure(
            StatusCode::BAD_REQUEST,
            "upstream_context_too_large",
            false
        ));
        assert!(!retryable_failure(
            StatusCode::FORBIDDEN,
            "upstream_content_policy",
            false
        ));
        assert_eq!(
            AttemptFailure::status_with_body(
                StatusCode::BAD_REQUEST,
                Some(br#"{"error":{"code":"model_at_capacity"}}"#)
            )
            .status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            canonical_upstream_status(StatusCode::FORBIDDEN, "upstream_quota_exhausted"),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            canonical_upstream_status(StatusCode::TOO_MANY_REQUESTS, "upstream_usage_not_included"),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn local_errors_use_openai_compatible_error_types() {
        assert_eq!(
            api_error_type(StatusCode::UNAUTHORIZED),
            "authentication_error"
        );
        assert_eq!(api_error_type(StatusCode::FORBIDDEN), "permission_error");
        assert_eq!(
            api_error_type(StatusCode::TOO_MANY_REQUESTS),
            "rate_limit_error"
        );
        assert_eq!(
            api_error_type(StatusCode::BAD_REQUEST),
            "invalid_request_error"
        );
        assert_eq!(api_error_type(StatusCode::BAD_GATEWAY), "server_error");
        assert_eq!(
            api_error_code("upstream_quota_exhausted"),
            "insufficient_quota"
        );
        assert_eq!(
            api_error_code("upstream_usage_not_included"),
            "usage_not_included"
        );
        assert_eq!(
            api_error_code("upstream_model_capacity"),
            "model_at_capacity"
        );
        assert_eq!(api_error_code("local_internal_code"), "local_internal_code");
    }

    #[test]
    fn streaming_terminal_errors_keep_the_canonical_category() {
        let terminal = parse_sse_event(
            br#"data: {"type":"response.failed","response":{"error":{"type":"usage_limit_reached","resets_in_seconds":7}}}

"#,
        );
        assert_eq!(terminal.error_category, Some("upstream_quota_exhausted"));
        assert_eq!(terminal.error_status, Some(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(terminal.cooldown_hint.retry_after_ms, Some(7_000));
        assert!(terminal.cooldown_hint.global);
    }

    #[test]
    fn responses_lite_keeps_only_client_executed_tools() {
        let mut request = json!({
            "model": "gpt-lite",
            "tools": [
                {"type": "function", "name": "lookup"},
                {"type": "custom", "name": "patch"},
                {"type": "tool_search", "execution": "client"},
                {"type": "tool_search", "execution": "server"},
                {"type": "web_search"},
                {"type": "image_generation"}
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "tools": [
                    {"type": "function", "name": "lookup"},
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
        assert_eq!(types("/tools"), ["function", "custom", "tool_search"]);
        assert_eq!(types("/tool_choice/tools"), ["function"]);
        assert_eq!(types("/input/0/tools"), ["custom"]);
        assert_eq!(types("/response/tools"), ["function"]);
        assert_eq!(request["input"].as_array().unwrap().len(), 2);
        assert!(request.pointer("/response/tool_choice").is_none());
        assert_eq!(request["parallel_tool_calls"], false);
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
    fn chat_translation_synthesizes_missing_tool_call_ids() {
        let translated = translate_chat_response(
            br#"{"id":"chatcmpl_1","model":"model","choices":[{"index":2,"message":{"tool_calls":[{"type":"function","function":{"name":"lookup","arguments":"{}"}}]}}]}"#,
        )
        .unwrap_or_else(|_| panic!("chat response should translate"));
        let response: Value = serde_json::from_slice(&translated).unwrap();

        assert_eq!(
            response.pointer("/output/0/id").unwrap(),
            "call_chatcmpl_1_2_0"
        );
        assert_eq!(
            response.pointer("/output/0/call_id").unwrap(),
            "call_chatcmpl_1_2_0"
        );
    }

    #[test]
    fn oversized_sse_event_is_recorded_as_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            UsageEvent {
                request_id: "request".into(),
                attempt: 1,
                local_key_id: "key".into(),
                source_id: "source".into(),
                candidate_id: Some("source".into()),
                account_id: None,
                routing: None,
                requested_model: Some("model".into()),
                resolved_model: Some("model".into()),
                wire_api: crate::WireApi::Responses,
                success: true,
                http_status: 200,
                error_category: None,
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: Some(0),
                latency_ms: 0,
                ttft_ms: None,
                generation_ms: None,
                input_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_tokens: None,
                output_tokens: None,
                total_tokens: None,
            },
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.ingest_sse(&vec![b'x'; MAX_SSE_EVENT_BYTES + 1]);
        assert!(stream.terminated);
        assert!(stream.sse_pending.is_empty());
        let failure =
            String::from_utf8(stream.output_pending.pop_front().unwrap().to_vec()).unwrap();
        assert!(failure.starts_with("event: response.failed\ndata: "));
        assert!(failure.contains("\"code\":\"stream_event_too_large\""));
        assert!(stream.output_pending.is_empty());
        drop(stream);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("stream_event_too_large")
        );
    }

    #[test]
    fn non_stream_usage_normalizes_cached_reasoning_and_total_tokens() {
        let mut event = test_usage_event();
        populate_tokens(
            &mut event,
            br#"{"response":{"response":{"usage":{"input_tokens":16,"input_tokens_details":{"cached_tokens":30},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":30},"total_tokens":10}}}}"#,
        );

        assert_eq!(event.input_tokens, Some(16));
        assert_eq!(event.cached_input_tokens, Some(16));
        assert_eq!(event.reasoning_tokens, Some(5));
        assert_eq!(event.output_tokens, Some(5));
        assert_eq!(event.total_tokens, Some(21));
    }

    #[test]
    fn streaming_chat_usage_captures_cached_prompt_tokens() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        stream.ingest_sse(
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"prompt_tokens\":32,\"prompt_tokens_details\":{\"cached_tokens\":9,\"cache_write_tokens\":7},\"completion_tokens\":6,\"completion_tokens_details\":{\"reasoning_tokens\":4}}}}\n\n",
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(32));
        assert_eq!(events[0].cached_input_tokens, Some(9));
        assert_eq!(events[0].cache_write_input_tokens, Some(7));
        assert_eq!(events[0].reasoning_tokens, Some(4));
        assert_eq!(events[0].output_tokens, Some(6));
        assert_eq!(events[0].total_tokens, Some(38));
    }

    #[test]
    fn all_responses_error_terminal_types_are_failures() {
        for event_type in [
            "response.failed",
            "response.incomplete",
            "response.cancelled",
            "response.canceled",
            "error",
        ] {
            let event = format!("data: {{\"type\":\"{event_type}\"}}\n\n");
            assert_eq!(
                parse_sse_event(event.as_bytes()).outcome,
                Some(TerminalOutcome::Failure)
            );
        }
    }

    #[test]
    fn ttft_requires_real_output_for_supported_stream_protocols() {
        for event in [
            "data: {\"type\":\"response.created\"}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"hidden\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null}}\n\n",
        ] {
            assert!(!parse_sse_event(event.as_bytes()).has_output_delta);
        }
        for event in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
        ] {
            assert!(parse_sse_event(event.as_bytes()).has_output_delta);
        }
    }

    #[test]
    fn retry_after_supports_delta_seconds_and_http_dates() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, reqwest::header::HeaderValue::from_static("17"));
        assert_eq!(retry_after_ms(&headers, now), Some(17_000));

        headers.insert(
            RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("518400"),
        );
        assert_eq!(retry_after_ms(&headers, now), Some(518_400_000));

        let date = httpdate::fmt_http_date(now + Duration::from_secs(23));
        headers.insert(RETRY_AFTER, date.parse().unwrap());
        assert_eq!(retry_after_ms(&headers, now), Some(23_000));
    }

    #[test]
    fn rate_limit_body_hint_uses_reset_time_and_marks_usage_limits_global() {
        let hint = rate_limit_body_hint_at(
            br#"{"error":{"type":"usage_limit_reached","resets_at":1700000120,"resets_in_seconds":1}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(hint.retry_after_ms, Some(120_000));
        assert!(hint.global);
    }

    #[test]
    fn rate_limit_body_hint_accepts_relative_reset_seconds() {
        let hint = rate_limit_body_hint_at(
            br#"{"error":{"code":"rate_limit","resets_in_seconds":"17"}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(hint.retry_after_ms, Some(17_000));
        assert!(!hint.global);
    }

    #[test]
    fn rate_limit_body_hint_accepts_retry_after_and_message_delays() {
        let retry_after = rate_limit_body_hint_at(
            br#"{"error":{"code":"rate_limit_exceeded","retry_after":"2.5"}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(retry_after.retry_after_ms, Some(2_500));

        let seconds = rate_limit_body_hint_at(
            br#"{"response":{"error":{"code":"rate_limit_exceeded","message":"Please try again in 11.054s."}}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(seconds.retry_after_ms, Some(11_054));

        let millis = rate_limit_body_hint_at(
            br#"{"error":{"message":"Please try again in 250ms."}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(millis.retry_after_ms, Some(250));
    }

    #[test]
    fn rate_limit_body_hint_accepts_top_level_quota_variants() {
        let hint = rate_limit_body_hint_at(
            br#"{"code":"rate_limit_reached","resets_in_seconds":9}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(hint.retry_after_ms, Some(9_000));
        assert!(hint.global);
    }

    #[test]
    fn websocket_connection_limit_is_account_global() {
        let hint = rate_limit_body_hint_at(
            br#"{"error":{"code":"websocket_connection_limit_reached"}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert!(hint.global);
    }

    #[test]
    fn rate_limit_delay_uses_the_stronger_hint_and_keeps_explicit_zero() {
        assert_eq!(
            rate_limit_cooldown_ms(Some(1_000), Some(120_000), 1),
            120_000
        );
        assert_eq!(rate_limit_cooldown_ms(Some(0), None, 5), 0);
    }

    #[test]
    fn no_header_rate_limit_backoff_is_exponential_and_capped() {
        assert_eq!(exponential_backoff_ms(1), 1_000);
        assert_eq!(exponential_backoff_ms(2), 2_000);
        assert_eq!(exponential_backoff_ms(3), 4_000);
        assert_eq!(exponential_backoff_ms(32), MAX_RATE_LIMIT_COOLDOWN_MS);
    }

    #[test]
    fn failed_half_open_probes_back_off_without_shortening_retry_after() {
        assert_eq!(half_open_backoff_ms(0, 2, true), 2_000);
        assert_eq!(half_open_backoff_ms(60_000, 2, false), 60_000);
        assert_eq!(half_open_backoff_ms(60_000, 2, true), 120_000);
        assert_eq!(half_open_backoff_ms(60_000, 3, true), 240_000);
        assert_eq!(
            half_open_backoff_ms(60_000, 32, true),
            MAX_RATE_LIMIT_COOLDOWN_MS
        );
        assert_eq!(
            half_open_backoff_ms(MAX_RATE_LIMIT_RETRY_HINT_MS, 2, true),
            MAX_RATE_LIMIT_RETRY_HINT_MS
        );
    }

    fn test_usage_event() -> UsageEvent {
        UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "source".into(),
            candidate_id: Some("source".into()),
            account_id: None,
            routing: None,
            requested_model: Some("model".into()),
            resolved_model: Some("model".into()),
            wire_api: crate::WireApi::Responses,
            success: true,
            http_status: 200,
            error_category: None,
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 0,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
        }
    }
}

use super::super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::super::continuation::{
    prepare_response_continuation, RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
};
use super::super::errors::api_error;
use super::super::now_ms;
use super::super::request::{
    candidate_protocols, chat_request_has_supported_content, client_request_identity,
    codex_background_request_kind, forwarded_codex_headers, forwarded_messages_headers,
    is_managed_codex_client, normalize_codex_reasoning_placeholder, request_id, ServiceTierPolicy,
    CODEX_RESPONSES_LITE_HEADER,
};
use super::request::{execute_request, RequestExecution, ReservedCandidate};
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::scheduler::rotation::SharedRequestBudget;
use crate::{GatewayRuntime, UsageTransport, WireApi};
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;

/// WebSocket -> HTTP fallback carries the same identity and physical
/// dispatch counter; a transport change is not a new client request.
#[derive(Clone)]
pub(in crate::gateway) struct RoutedRequestIdentity {
    pub(in crate::gateway) request_id: String,
    pub(in crate::gateway) budget: SharedRequestBudget,
    pub(in crate::gateway) transport: UsageTransport,
    pub(in crate::gateway) transport_fallback: Option<crate::usage::TransportFallbackReason>,
    pub(in crate::gateway) service_tier_policy: ServiceTierPolicy,
}

/// Internal compact-to-Responses dispatch is not an ordinary conversation turn.
#[derive(Clone, Copy)]
pub(in crate::gateway) struct RoutedCompactionRequest;

pub(in crate::gateway) async fn execute_client_request(
    runtime: Arc<GatewayRuntime>,
    http_request: Request<Body>,
    client_wire_api: WireApi,
) -> Response<Body> {
    execute_reserved_client_request(runtime, http_request, client_wire_api, None).await
}

pub(in crate::gateway) async fn execute_reserved_client_request(
    runtime: Arc<GatewayRuntime>,
    http_request: Request<Body>,
    client_wire_api: WireApi,
    reserved_candidate: Option<ReservedCandidate>,
) -> Response<Body> {
    // The HTTP attempt future is large; a transport handoff must not multiply
    // its stack footprint inside the WebSocket connection task.
    Box::pin(execute_client_request_inner(
        runtime,
        http_request,
        client_wire_api,
        None,
        false,
        reserved_candidate,
    ))
    .await
}

pub(in crate::gateway) async fn execute_gemini_client_request(
    runtime: Arc<GatewayRuntime>,
    http_request: Request<Body>,
    model: String,
    force_stream: bool,
) -> Response<Body> {
    execute_client_request_inner(
        runtime,
        http_request,
        WireApi::Gemini,
        Some(model),
        force_stream,
        None,
    )
    .await
}

async fn execute_client_request_inner(
    runtime: Arc<GatewayRuntime>,
    http_request: Request<Body>,
    client_wire_api: WireApi,
    path_model: Option<String>,
    force_stream: bool,
    reserved_candidate: Option<ReservedCandidate>,
) -> Response<Body> {
    let (parts, request_body) = http_request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let key = super::super::auth::authenticate_client(&runtime, &headers, client_wire_api);
    let Some(key) = key else {
        return unauthorized();
    };
    let client_api = match client_wire_api {
        WireApi::Responses => ClientWireApi::Responses,
        WireApi::ChatCompletions => ClientWireApi::ChatCompletions,
        WireApi::Messages => ClientWireApi::Messages,
        WireApi::Gemini => ClientWireApi::Gemini,
        WireApi::GeminiInteractions => ClientWireApi::GeminiInteractions,
    };
    if !runtime.allows_client_wire_api(&key, client_api) {
        return client_api_forbidden();
    }
    let mut request_json =
        match super::super::request_body::read_json_object(&headers, request_body).await {
            Ok(request_object) => Value::Object(request_object),
            Err(error_response) => return *error_response,
        };
    if client_wire_api == WireApi::GeminiInteractions {
        if let Err(message) = crate::protocol::interactions::validate_request(&request_json) {
            return api_error(
                StatusCode::BAD_REQUEST,
                message,
                error_codes::INVALID_REQUEST,
            );
        }
    }
    runtime.refresh_basis_points_access(&key).await;
    let mut tool_policy = parts
        .extensions
        .get::<super::super::request::RequestToolPolicy>()
        .cloned()
        .unwrap_or_else(|| super::super::request::RequestToolPolicy::new(&runtime, &request_json));
    let managed_codex_client = is_managed_codex_client(&headers);
    if client_wire_api == WireApi::Responses {
        normalize_codex_reasoning_placeholder(&runtime, &key, &headers, &mut request_json);
    }
    let captured_tier_policy = parts
        .extensions
        .get::<RoutedRequestIdentity>()
        .map(|identity| identity.service_tier_policy.clone());
    let mut service_tier_policy = captured_tier_policy.clone().unwrap_or_else(|| {
        if managed_codex_client {
            ServiceTierPolicy::pool_owned(&request_json)
        } else {
            ServiceTierPolicy::client_owned(&request_json)
        }
    });
    if client_wire_api == WireApi::ChatCompletions
        && !chat_request_has_supported_content(&request_json)
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Chat Completions supports text, image and file content only",
            error_codes::CHAT_FEATURE_NOT_SUPPORTED,
        );
    }
    let body_model = request_json
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string);
    if let (Some(path_model), Some(body_model)) = (path_model.as_deref(), body_model.as_deref()) {
        if !path_model.eq_ignore_ascii_case(body_model) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "model in the path must match model in the request body",
                error_codes::INVALID_REQUEST,
            );
        }
    }
    let Some(requested_model) = path_model.or(body_model) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a non-empty string",
            error_codes::INVALID_REQUEST,
        );
    };
    if client_wire_api == WireApi::Gemini {
        // Bind path-only continuations to their public model; native encoding removes this field.
        request_json["model"] = Value::String(requested_model.clone());
    }
    let background_kind = (client_wire_api == WireApi::Responses)
        .then(|| codex_background_request_kind(&headers, &request_json))
        .flatten();
    let identity = parts.extensions.get::<RoutedRequestIdentity>().cloned();
    let transport = identity
        .as_ref()
        .map(|identity| identity.transport)
        .unwrap_or(UsageTransport::Http);
    let transport_fallback = identity
        .as_ref()
        .and_then(|identity| identity.transport_fallback);
    let (request_id, budget) = identity.map_or_else(
        || {
            (
                request_id(),
                SharedRequestBudget::for_incoming_request(runtime.request_dispatch_budget()),
            )
        },
        |identity| (identity.request_id, identity.budget),
    );
    let stream = match request_json.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                error_codes::INVALID_REQUEST,
            )
        }
        None => false,
    } || force_stream;
    if let Some(kind) = background_kind {
        runtime.mark_request_origin(&request_id, kind);
        if !runtime.codex_background_request_enabled(kind) {
            runtime.blocked_codex_background_event(
                &request_id,
                &key.id,
                &requested_model,
                client_wire_api,
                UsageTransport::Http,
                kind,
            );
            return blocked_background_response(client_wire_api, stream, &request_id, kind);
        }
    }
    let mut client_identity = client_request_identity(&headers, &request_json);
    if client_identity.affinity_id.is_none()
        && !request_json
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.trim().is_empty())
    {
        client_identity.affinity_id =
            crate::gateway::request::history_affinity_fingerprint(&key.id, &request_json);
    }
    let client_context_id = client_identity.context_id;
    if client_wire_api == WireApi::Responses
        && background_kind.is_none()
        && parts.extensions.get::<RoutedCompactionRequest>().is_none()
    {
        tool_policy.capture_cache_context(
            &runtime,
            &request_json,
            &key.id,
            &request_id,
            client_context_id.as_deref(),
        );
    }
    let continuation = match client_wire_api {
        WireApi::GeminiInteractions => Some(crate::gateway::continuation::interactions::prepare(
            &runtime,
            &key.id,
            &request_json,
            now_ms(),
        )),
        WireApi::Responses => Some(prepare_response_continuation(
            &runtime,
            &key.id,
            &mut request_json,
            now_ms(),
            None,
        )),
        WireApi::Gemini => Some(crate::gateway::continuation::gemini::prepare(
            &runtime,
            &key.id,
            &request_json,
            now_ms(),
        )),
        _ => None,
    }
    .transpose();
    let continuation = match continuation {
        Ok(continuation) => continuation,
        Err(()) => {
            return api_error(
                StatusCode::CONFLICT,
                RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
                RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
            )
        }
    };

    let resolved_model = runtime
        .resolve_visible_model(
            &key,
            &requested_model,
            candidate_protocols(client_wire_api),
            now_ms(),
        )
        .or_else(|| {
            let resolved = runtime.resolve_configured_model(
                &key,
                &requested_model,
                candidate_protocols(client_wire_api),
            )?;
            (runtime.route_recovery_enabled()
                || runtime.basis_points_access_failed_for_model(
                    &key,
                    &resolved,
                    candidate_protocols(client_wire_api),
                ))
            .then_some(resolved)
        });
    let Some(resolved_model) = resolved_model else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            error_codes::MODEL_NOT_FOUND,
        );
    };
    if captured_tier_policy.is_none() {
        service_tier_policy.capture_pool_default(
            &runtime,
            &resolved_model,
            &runtime.client_service_tiers(&key, &resolved_model, client_wire_api),
        );
    }
    let responses_lite = (client_wire_api == WireApi::Responses)
        .then(|| headers.get(CODEX_RESPONSES_LITE_HEADER).cloned())
        .flatten();
    let forwarded_headers = match client_wire_api {
        WireApi::Messages => forwarded_messages_headers(&headers),
        WireApi::Responses | WireApi::ChatCompletions => {
            forwarded_codex_headers(&headers, &request_id)
        }
        WireApi::Gemini | WireApi::GeminiInteractions => {
            super::super::request::forwarded_bridge_gemini_headers(&headers)
        }
    };
    execute_request(RequestExecution {
        tool_policy,
        runtime: runtime.clone(),
        key,
        request: request_json,
        service_tier_policy,
        requested_model,
        resolved_model,
        stream,
        request_id,
        forwarded_headers,
        client_context_id,
        client_affinity_id: client_identity.affinity_id,
        response_affinity_key: continuation
            .as_ref()
            .and_then(|continuation| continuation.response_affinity_key.clone()),
        requires_affinity_owner: continuation
            .is_some_and(|continuation| continuation.requires_affinity_owner),
        client_wire_api,
        responses_lite,
        allow_previous_response_reset: true,
        attempt_offset: 0,
        budget,
        transport,
        transport_fallback,
        reserved_candidate,
    })
    .await
}

fn blocked_background_response(
    client_wire_api: WireApi,
    stream: bool,
    request_id: &str,
    kind: &str,
) -> Response<Body> {
    // Empty success would let the client mark a memory job complete without data.
    let code = match kind {
        "memory_extraction" => Some(error_codes::CODEX_BACKGROUND_BLOCKED_MEMORY_EXTRACTION),
        "memory_consolidation" => Some(error_codes::CODEX_BACKGROUND_BLOCKED_MEMORY_CONSOLIDATION),
        _ => None,
    };
    if let Some(code) = code {
        return api_error(
            StatusCode::FORBIDDEN,
            "This memory stage is disabled in Relay",
            code,
        );
    }
    let response_id = format!("resp_relay_blocked_{request_id}");
    let response_body = if stream {
        format!(
            "event: response.completed\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.completed",
                "response": {"id": response_id, "object": "response", "status": "completed", "output": [], "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}, "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}}
            })
        )
    } else {
        match client_wire_api {
            WireApi::Responses => serde_json::json!({
                "id": response_id,
                "object": "response",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
                "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}
            })
            .to_string(),
            WireApi::ChatCompletions => serde_json::json!({"id": response_id, "object": "chat.completion", "choices": [], "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}}).to_string(),
            WireApi::Messages => serde_json::json!({"id": response_id, "type": "message", "role": "assistant", "content": [], "stop_reason": "end_turn", "usage": {"input_tokens": 0, "output_tokens": 0}}).to_string(),
            WireApi::Gemini => serde_json::json!({"candidates": [], "usageMetadata": {"promptTokenCount": 0, "candidatesTokenCount": 0, "totalTokenCount": 0}}).to_string(),
            WireApi::GeminiInteractions => serde_json::json!({"id":response_id,"status":"completed","outputs":[],"usage":{"total_input_tokens":0,"total_output_tokens":0,"total_tokens":0}}).to_string(),
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(
            CONTENT_TYPE,
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(Body::from(response_body))
        .expect("blocked response builder is valid")
}

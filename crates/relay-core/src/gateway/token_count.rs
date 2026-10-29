//! Native, non-generating token counts. A generation route is not evidence of
//! a tokenizer for another provider: unsupported routes never estimate or infer.

use super::auth::{
    authenticate_client, client_api_forbidden, invalid_host, unauthorized, valid_local_host,
};
use super::errors::{api_error, api_error_with_origin_and_category};
use crate::protocol::ClientWireApi;
use crate::runtime::{
    AuthenticatedKey, AuthorizationDispatch, AuthorizationIdentityPolicy, ExecutorRoute,
};
use crate::scheduler::rotation::{
    AttemptObservation, ExecutionObservation, HealthObservation, RotationOperation,
    SharedRequestBudget,
};
use crate::{GatewayRuntime, WireApi, error_codes};
use axum::body::Body;
use axum::extract::State;
use axum::http::{
    HeaderMap, Request, Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, RETRY_AFTER},
};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

const MAX_COUNT_RESPONSE_BYTES: usize = 256 * 1024;
const COUNT_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn messages(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    super::messages::native_messages_error_response(
        count(runtime, request, WireApi::Messages, None).await,
    )
    .await
}

pub(super) async fn gemini(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    model: &str,
) -> Response<Body> {
    count(
        runtime,
        request,
        WireApi::Gemini,
        Some(model.strip_prefix("models/").unwrap_or(model)),
    )
    .await
}

async fn count(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    protocol: WireApi,
    path_model: Option<&str>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    if !valid_local_host(&parts.headers) {
        return invalid_host();
    }
    let Some(key) = authenticate_client(&runtime, &parts.headers, protocol) else {
        return unauthorized();
    };
    let client_api = if protocol == WireApi::Messages {
        ClientWireApi::Messages
    } else {
        ClientWireApi::Gemini
    };
    if !runtime.allows_client_wire_api(&key, client_api) {
        return client_api_forbidden();
    }
    let body = match super::request_body::read_json_object(&parts.headers, body).await {
        Ok(body) => Value::Object(body),
        Err(error) => return *error,
    };
    let model = path_model.or_else(|| body.get("model").and_then(Value::as_str));
    let Some(model) = model.filter(|model| crate::is_valid_model_id(model)) else {
        return invalid_request("model must be a valid non-empty identifier");
    };
    if body.get("stream").is_some()
        || body
            .get("model")
            .is_some_and(|value| value.as_str().is_none_or(|value| !same_model(value, model)))
        || (protocol == WireApi::Gemini
            && body
                .pointer("/generateContentRequest/model")
                .is_some_and(|value| value.as_str().is_none_or(|value| !same_model(value, model))))
    {
        return invalid_request(
            "token counting is non-streaming and all supplied models must match the requested model",
        );
    }
    if protocol == WireApi::Gemini
        && (body
            .get("generateContentRequest")
            .is_some_and(|value| !value.is_object())
            || (body.get("generateContentRequest").is_some() && body.get("contents").is_some()))
    {
        return invalid_request("use contents or an object generateContentRequest, not both");
    }
    let Some(resolved_model) = runtime.resolve_model(&key, model) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            error_codes::MODEL_NOT_FOUND,
        );
    };
    match tokio::time::timeout(
        COUNT_TIMEOUT,
        execute(
            &runtime,
            &key,
            protocol,
            &resolved_model,
            body,
            &parts.headers,
        ),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "token counting timed out",
            error_codes::UPSTREAM_GATEWAY_TIMEOUT,
        ),
    }
}

fn same_model(value: &str, model: &str) -> bool {
    value
        .strip_prefix("models/")
        .unwrap_or(value)
        .eq_ignore_ascii_case(model)
}

fn invalid_request(message: &str) -> Response<Body> {
    api_error(
        StatusCode::BAD_REQUEST,
        message,
        error_codes::INVALID_REQUEST,
    )
}

fn upstream_error(status: StatusCode, message: &str, code: &str) -> Response<Body> {
    api_error_with_origin_and_category(
        status,
        message,
        code,
        code,
        crate::ErrorOrigin::Provider,
        None,
    )
}

fn native_route(route: &ExecutorRoute, protocol: WireApi) -> bool {
    route.account_id.is_none()
        && route.client_wire_api == protocol
        && route.adapter.is_passthrough()
}

fn count_url(route: &ExecutorRoute, protocol: WireApi) -> Option<url::Url> {
    let mut url = route.upstream_url.clone();
    let path = if protocol == WireApi::Messages {
        format!(
            "{}/messages/count_tokens",
            url.path().strip_suffix("/messages")?
        )
    } else {
        format!(
            "{}:countTokens",
            url.path().strip_suffix(":generateContent")?
        )
    };
    url.set_path(&path);
    Some(url)
}

async fn execute(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    protocol: WireApi,
    model: &str,
    mut body: Value,
    headers: &HeaderMap,
) -> Response<Body> {
    let routes = runtime.configured_executor_routes(key, model, &[protocol], false);
    let eligible: HashSet<_> = routes
        .iter()
        .filter(|route| native_route(route, protocol) && count_url(route, protocol).is_some())
        .map(|route| route.candidate_id.clone())
        .collect();
    if eligible.is_empty() {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "token counting requires a native API source with the requested counting endpoint; account transports and cross-protocol estimates are not supported",
            error_codes::TOKEN_COUNT_UNSUPPORTED,
        );
    }
    let excluded: HashSet<_> = routes
        .into_iter()
        .map(|route| route.candidate_id)
        .filter(|id| !eligible.contains(id))
        .collect();
    let budget = SharedRequestBudget::for_incoming_request(runtime.request_dispatch_budget());
    let Some((_, lease)) = runtime
        .select_and_reserve_operation_with_budget(
            key,
            model,
            &[protocol],
            &excluded,
            (None, None),
            super::now_ms(),
            RotationOperation::Metadata,
            &budget,
        )
        .await
    else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no native token-counting source is currently available",
            error_codes::NO_ELIGIBLE_SOURCE,
        );
    };
    let route = runtime
        .executor_routes(
            lease.candidate_id(),
            model,
            &key.scope_snapshot(),
            &[protocol],
            false,
        )
        .into_iter()
        .find(|route| native_route(route, protocol));
    let Some(route) = route else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "token-counting source is no longer available",
            error_codes::NO_ELIGIBLE_SOURCE,
        );
    };
    let Some(url) = count_url(&route, protocol) else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "token-counting endpoint is unavailable",
            error_codes::TOKEN_COUNT_UNSUPPORTED,
        );
    };
    if protocol == WireApi::Messages {
        body["model"] = Value::String(route.source_model.clone());
    } else {
        body.as_object_mut()
            .expect("validated object")
            .remove("model");
        if let Some(request) = body.get_mut("generateContentRequest") {
            request["model"] = Value::String(format!("models/{}", route.source_model));
        }
    }
    let forwarded = if protocol == WireApi::Messages {
        super::request::forwarded_messages_headers(headers)
    } else {
        super::request::forwarded_bridge_gemini_headers(headers)
    };
    let request = runtime
        .request_client(&route.candidate_id)
        .post(url)
        .headers(route.upstream_headers.clone())
        .headers(forwarded)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .body(serde_json::to_vec(&body).expect("validated JSON"));
    let upstream = runtime
        .send_authorized_request_for_route(
            &route,
            request,
            AuthorizationDispatch {
                client_version: None,
                identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                turn_scope: None,
                budget: Some(&budget),
                lease: Some(&lease),
                usage_event: None,
            },
        )
        .await;
    let response = match upstream {
        Ok(upstream) => response(upstream.response, protocol).await,
        Err(_) => upstream_error(
            StatusCode::BAD_GATEWAY,
            "token-counting upstream request failed",
            error_codes::UPSTREAM_ERROR,
        ),
    };
    if lease.has_dispatched() {
        let _ = lease.settle_rotation(
            AttemptObservation {
                execution: if response.status().is_success() {
                    ExecutionObservation::committed()
                } else {
                    ExecutionObservation::unknown()
                },
                // A successful tokenizer does not establish generation health.
                health: HealthObservation::Unknown,
            },
            super::now_ms(),
        );
    }
    // No generation usage event, continuation binding or health success is created.
    response
}

async fn response(upstream: reqwest::Response, protocol: WireApi) -> Response<Body> {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    if !status.is_success() {
        // Never echo an upstream error body: it can contain prompt text or credentials.
        let mut response = upstream_error(
            status,
            "upstream token-counting endpoint rejected the request",
            error_codes::UPSTREAM_ERROR,
        );
        if let Some(retry_after) = headers.get(RETRY_AFTER) {
            response
                .headers_mut()
                .insert(RETRY_AFTER, retry_after.clone());
        }
        return response;
    }
    let count_field = if protocol == WireApi::Messages {
        "input_tokens"
    } else {
        "totalTokens"
    };
    match crate::catalog_io::response_json(upstream, MAX_COUNT_RESPONSE_BYTES).await {
        Ok(body) if body.get(count_field).and_then(Value::as_u64).is_some() => {
            super::response::proxy_response(
                status,
                &headers,
                Body::from(serde_json::to_vec(&body).expect("validated JSON")),
            )
        }
        _ => upstream_error(
            StatusCode::BAD_GATEWAY,
            "upstream returned an invalid token count",
            error_codes::TOKEN_COUNT_RESPONSE_INVALID,
        ),
    }
}

use super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::errors::api_error;
use crate::{error_codes, protocol::ClientWireApi, GatewayRuntime, WireApi};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode, Uri},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn catalog_protocol(headers: &HeaderMap) -> Option<WireApi> {
    if headers.contains_key("x-goog-api-key") {
        Some(WireApi::Gemini)
    } else if headers.contains_key("x-api-key") || headers.contains_key("anthropic-version") {
        Some(WireApi::Messages)
    } else {
        None
    }
}

pub(super) fn native_catalog(
    runtime: &GatewayRuntime,
    headers: &HeaderMap,
    protocol: WireApi,
    model: Option<&str>,
) -> Response<Body> {
    if !valid_local_host(headers) {
        return invalid_host();
    }
    let Some(key) = super::auth::authenticate_client(runtime, headers, protocol) else {
        return unauthorized();
    };
    let client = if protocol == WireApi::Gemini {
        ClientWireApi::Gemini
    } else {
        ClientWireApi::Messages
    };
    if !runtime.allows_client_wire_api(&key, client) {
        return client_api_forbidden();
    }
    let models = runtime.visible_models(&key, &[protocol], super::now_ms());
    let entry = |id: &str| -> Value {
        if protocol == WireApi::Gemini {
            let model = runtime.resolve_model(&key, id).unwrap_or_else(|| id.into());
            let streaming = runtime
                .configured_executor_routes(&key, &model, &[protocol], true)
                .iter()
                .any(|route| {
                    route.account_id.is_some()
                        || runtime
                            .route_capabilities(&route.candidate_id, &route.source_model)
                            .and_then(|capability| {
                                capability.features.get(&crate::ProtocolFeature::Streaming)
                            })
                            .is_some_and(|status| status.available())
                });
            let mut methods = vec!["generateContent"];
            if streaming {
                methods.push("streamGenerateContent");
            }
            json!({"name":format!("models/{id}"),"displayName":id,"supportedGenerationMethods":methods})
        } else {
            json!({"id":id,"display_name":id,"type":"model"})
        }
    };
    if let Some(model) = model {
        return match models.iter().find(|id| id.eq_ignore_ascii_case(model)) {
            Some(id) => Json(entry(id)).into_response(),
            None => api_error(
                StatusCode::NOT_FOUND,
                "model is not available in this managed pool",
                error_codes::MODEL_NOT_FOUND,
            ),
        };
    }
    if protocol == WireApi::Gemini {
        Json(json!({"models":models.iter().map(|id| entry(id)).collect::<Vec<_>>()}))
            .into_response()
    } else {
        Json(json!({"data":models.iter().map(|id| entry(id)).collect::<Vec<_>>(),"has_more":false,"first_id":models.first(),"last_id":models.last()})).into_response()
    }
}

pub(super) async fn gemini_models(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
) -> Response<Body> {
    native_catalog(&runtime, &headers, WireApi::Gemini, None)
}

pub(super) async fn native_model(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    Path(model): Path<String>,
    uri: Uri,
) -> Response<Body> {
    if uri.path().starts_with("/v1beta/") {
        return native_catalog(&runtime, &headers, WireApi::Gemini, Some(&model));
    }
    if let Some(protocol) = catalog_protocol(&headers) {
        return native_catalog(&runtime, &headers, protocol, Some(&model));
    }
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = super::auth::authenticate_client(&runtime, &headers, WireApi::Responses) else {
        return unauthorized();
    };
    let protocols = [WireApi::Responses, WireApi::ChatCompletions]
        .into_iter()
        .filter(|protocol| {
            let client = if *protocol == WireApi::Responses {
                ClientWireApi::Responses
            } else {
                ClientWireApi::ChatCompletions
            };
            runtime.allows_client_wire_api(&key, client)
        })
        .collect::<Vec<_>>();
    if protocols.is_empty() {
        return client_api_forbidden();
    }
    if runtime
        .visible_models(&key, &protocols, super::now_ms())
        .iter()
        .any(|id| id.eq_ignore_ascii_case(&model))
    {
        Json(json!({"id": model, "object":"model", "owned_by":"zenith-relay"})).into_response()
    } else {
        api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            error_codes::MODEL_NOT_FOUND,
        )
    }
}

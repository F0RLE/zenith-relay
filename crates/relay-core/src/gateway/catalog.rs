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

pub(super) async fn native_catalog(
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
    runtime.refresh_basis_points_access(&key).await;
    let models = runtime.visible_models(&key, &[protocol], super::now_ms());
    let build_catalog_entry = |model_id: &str| -> Value {
        if protocol == WireApi::Gemini {
            let resolved_model = runtime
                .resolve_model(&key, model_id)
                .unwrap_or_else(|| model_id.into());
            let streaming = !runtime
                .configured_executor_routes(&key, &resolved_model, &[protocol], true)
                .is_empty();
            let mut methods = vec!["generateContent"];
            if streaming {
                methods.push("streamGenerateContent");
            }
            json!({"name":format!("models/{model_id}"),"displayName":model_id,"supportedGenerationMethods":methods})
        } else {
            json!({"id":model_id,"display_name":model_id,"type":"model"})
        }
    };
    if let Some(model) = model {
        return match models
            .iter()
            .find(|model_id| model_id.eq_ignore_ascii_case(model))
        {
            Some(model_id) => Json(build_catalog_entry(model_id)).into_response(),
            None => api_error(
                StatusCode::NOT_FOUND,
                "model is not available in this managed pool",
                error_codes::MODEL_NOT_FOUND,
            ),
        };
    }
    if protocol == WireApi::Gemini {
        Json(json!({"models":models.iter().map(|model_id| build_catalog_entry(model_id)).collect::<Vec<_>>()}))
            .into_response()
    } else {
        Json(json!({"data":models.iter().map(|model_id| build_catalog_entry(model_id)).collect::<Vec<_>>(),"has_more":false,"first_id":models.first(),"last_id":models.last()})).into_response()
    }
}

pub(super) async fn gemini_models(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
) -> Response<Body> {
    native_catalog(&runtime, &headers, WireApi::Gemini, None).await
}

pub(super) async fn native_model(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    Path(model): Path<String>,
    uri: Uri,
) -> Response<Body> {
    if uri.path().starts_with("/v1beta/") {
        return native_catalog(&runtime, &headers, WireApi::Gemini, Some(&model)).await;
    }
    if let Some(protocol) = catalog_protocol(&headers) {
        return native_catalog(&runtime, &headers, protocol, Some(&model)).await;
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
    runtime.refresh_basis_points_access(&key).await;
    if runtime
        .visible_models(&key, &protocols, super::now_ms())
        .iter()
        .any(|model_id| model_id.eq_ignore_ascii_case(&model))
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

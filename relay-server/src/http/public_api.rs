use crate::state::AppState;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header::HOST, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use zenith_relay_core::{error_codes, ErrorOrigin};

pub async fn proxy(State(state): State<Arc<AppState>>, incoming_request: Request) -> Response {
    if !state.store.gateway_enabled().unwrap_or(false) {
        return unavailable(error_codes::GATEWAY_STOPPED);
    }
    let Ok(Some(runtime)) = state.runtime() else {
        return unavailable(error_codes::RUNTIME_UNAVAILABLE);
    };
    // relay-core is also used by the desktop loopback gateway. The server
    // invokes it in-process, so replace the untrusted public Host header with
    // a loopback authority at this internal boundary.
    let (mut parts, body) = incoming_request.into_parts();
    parts
        .headers
        .insert(HOST, HeaderValue::from_static("127.0.0.1"));
    let loopback_request = Request::from_parts(parts, body);
    zenith_relay_core::gateway::router(runtime)
        .oneshot(loopback_request)
        .await
        .unwrap_or_else(|_| unavailable("runtime_failure"))
}

pub async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": {
                "message": ErrorOrigin::Relay.prefix_message("route not found"),
                "type": "invalid_request_error",
                "code": error_codes::ROUTE_NOT_FOUND
            }
        })),
    )
        .into_response()
}

fn unavailable(code: &str) -> Response<Body> {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": {
                "message": ErrorOrigin::Relay.prefix_message("personal pool runtime is unavailable"),
                "type": "server_error",
                "code": code
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    #[tokio::test]
    async fn public_api_route_errors_use_the_relay_prefix() {
        let response = not_found().await;
        let response_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response_json: Value = serde_json::from_slice(&response_bytes).unwrap();

        assert_eq!(response_json["error"]["message"], "Relay: route not found");
        assert_eq!(response_json["error"]["code"], "route_not_found");
    }

    #[tokio::test]
    async fn public_api_runtime_errors_use_the_relay_prefix() {
        let response = unavailable("runtime_failure");
        let response_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response_json: Value = serde_json::from_slice(&response_bytes).unwrap();

        assert_eq!(
            response_json["error"]["message"],
            "Relay: personal pool runtime is unavailable"
        );
        assert_eq!(response_json["error"]["code"], "runtime_failure");
    }
}

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

pub async fn proxy(State(state): State<Arc<AppState>>, request: Request) -> Response {
    if !state.store.gateway_enabled().unwrap_or(false) {
        return unavailable("gateway_stopped");
    }
    let Ok(Some(runtime)) = state.runtime() else {
        return unavailable("runtime_unavailable");
    };
    // relay-core is also used by the desktop loopback gateway. The server
    // invokes it in-process, so replace the untrusted public Host header with
    // a loopback authority at this internal boundary.
    let (mut parts, body) = request.into_parts();
    parts
        .headers
        .insert(HOST, HeaderValue::from_static("127.0.0.1"));
    let request = Request::from_parts(parts, body);
    zenith_relay_core::gateway::router(runtime)
        .oneshot(request)
        .await
        .unwrap_or_else(|_| unavailable("runtime_failure"))
}

pub async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": {
                "message": "route not found",
                "type": "invalid_request_error",
                "code": "route_not_found"
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
                "message": "personal pool runtime is unavailable",
                "type": "server_error",
                "code": code
            }
        })),
    )
        .into_response()
}

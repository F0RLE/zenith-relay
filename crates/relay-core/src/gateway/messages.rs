use super::errors::LocalGatewayError;
use axum::body::Body;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, Response, StatusCode};
use serde_json::{json, Value};

const MAX_ERROR_MESSAGE_CHARS: usize = 1_024;

/// Normalizes only Relay's local error envelope for a native Claude Messages
/// client. Successful Messages bodies and SSE frames always pass through
/// unchanged.
pub(super) async fn native_messages_error_response(response: Response<Body>) -> Response<Body> {
    if response.status().is_success() || response.extensions().get::<LocalGatewayError>().is_none()
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let message = axum::body::to_bytes(body, MAX_ERROR_MESSAGE_CHARS.saturating_mul(4))
        .await
        .ok()
        .and_then(|body| native_messages_error_message(&body))
        .unwrap_or_else(|| "request failed".to_string());
    parts.headers.remove(CONTENT_LENGTH);
    // Relay accepts both Bearer and x-api-key locally, but a native Messages
    // client must not be told that only Bearer authentication is available.
    parts.headers.remove(WWW_AUTHENTICATE);
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let body = json!({
        "type": "error",
        "error": {
            "type": native_messages_error_type(parts.status),
            "message": message,
        }
    });
    Response::from_parts(parts, Body::from(body.to_string()))
}

fn native_messages_error_message(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body).ok().and_then(|body| {
        body.pointer("/error/message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(|message| message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect())
    })
}

fn native_messages_error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        status if status.as_u16() == 529 => "overloaded_error",
        status if status.is_server_error() => "api_error",
        _ => "invalid_request_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::errors::api_error;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn local_gateway_errors_use_the_native_messages_envelope() {
        let response = native_messages_error_response(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "all eligible sources are cooling down",
            "all_sources_cooling_down",
        ))
        .await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "rate_limit_error");
        assert_eq!(
            body["error"]["message"],
            "all eligible sources are cooling down"
        );
        assert!(body["error"].get("code").is_none());
    }

    #[tokio::test]
    async fn native_upstream_errors_are_preserved_verbatim() {
        let body = br#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens is required"}}"#;
        let response = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(CONTENT_TYPE, "application/json")
            .header("request-id", "req_native")
            .body(Body::from(body.as_slice()))
            .unwrap();

        let response = native_messages_error_response(response).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["request-id"], "req_native");
        let actual = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(actual.as_ref(), body);
    }
}

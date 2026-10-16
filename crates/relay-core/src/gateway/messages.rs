use super::errors::LocalGatewayError;
use axum::body::Body;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, Response, StatusCode};
use serde_json::{json, Value};

const MAX_ERROR_MESSAGE_CHARS: usize = 1_024;

/// Normalizes only Relay's local error envelope for a native Claude Messages
/// client. Successful Messages bodies and SSE frames always pass through
/// unchanged.
pub(super) async fn native_messages_error_response(
    gateway_response: Response<Body>,
) -> Response<Body> {
    if gateway_response.status().is_success()
        || gateway_response
            .extensions()
            .get::<LocalGatewayError>()
            .is_none()
    {
        return gateway_response;
    }
    let (mut parts, response_body) = gateway_response.into_parts();
    let origin = parts
        .headers
        .get("x-zenith-relay-error-origin")
        .and_then(|origin_header| origin_header.to_str().ok())
        .and_then(|origin_text| origin_text.parse().ok())
        .unwrap_or(crate::ErrorOrigin::Relay);
    let message = axum::body::to_bytes(response_body, MAX_ERROR_MESSAGE_CHARS.saturating_mul(4))
        .await
        .ok()
        .and_then(|body_bytes| native_messages_error_message(&body_bytes))
        .map(|message| origin.prefix_message(&message))
        .unwrap_or_else(|| origin.prefix_message("request failed"));
    parts.headers.remove(CONTENT_LENGTH);
    // Relay accepts both Bearer and x-api-key locally, but a native Messages
    // client must not be told that only Bearer authentication is available.
    parts.headers.remove(WWW_AUTHENTICATE);
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let error_response_body = json!({
        "type": "error",
        "error": {
            "type": native_messages_error_type(parts.status),
            "message": message,
        }
    });
    Response::from_parts(parts, Body::from(error_response_body.to_string()))
}

fn native_messages_error_message(response_body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(response_body)
        .ok()
        .and_then(|error_payload| {
            error_payload
                .pointer("/error/message")
                .or_else(|| error_payload.get("message"))
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
            crate::error_codes::ALL_SOURCES_COOLING_DOWN,
        ))
        .await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let response_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response_json: Value = serde_json::from_slice(&response_bytes).unwrap();
        assert_eq!(response_json["type"], "error");
        assert_eq!(response_json["error"]["type"], "rate_limit_error");
        assert_eq!(
            response_json["error"]["message"],
            "Relay: all eligible sources are cooling down"
        );
        assert!(response_json["error"].get("code").is_none());
    }

    #[tokio::test]
    async fn native_upstream_errors_are_preserved_verbatim() {
        let native_error_body = br#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens is required"}}"#;
        let response = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(CONTENT_TYPE, "application/json")
            .header("request-id", "req_native")
            .body(Body::from(native_error_body.as_slice()))
            .unwrap();

        let response = native_messages_error_response(response).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["request-id"], "req_native");
        let actual = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(actual.as_ref(), native_error_body);
    }
}

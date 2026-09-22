use super::*;
use crate::error_codes;
use crate::ErrorOrigin;

pub(super) async fn send_gateway_error(
    downstream: &mut WebSocket,
    failure: &GatewayFailure,
    request_id: Option<&str>,
) {
    let event = gateway_error_event(failure, request_id);
    let _ = downstream
        .send(Message::Text(event.to_string().into()))
        .await;
    let _ = downstream
        .send(Message::Close(Some(CloseFrame {
            code: if failure.status.is_client_error() {
                close_code::POLICY
            } else {
                close_code::ERROR
            },
            reason: "request failed".into(),
        })))
        .await;
}

pub(super) fn gateway_error_event(failure: &GatewayFailure, request_id: Option<&str>) -> Value {
    let code = super::super::errors::api_error_code(failure.category);
    json!({
        "type": "error",
        "status": failure.status.as_u16(),
        "error": {
            "type": super::super::errors::api_error_type(
                failure.status,
                code,
            ),
            "code": code,
            "message": failure.upstream_error.as_ref().and_then(|details| details.message.as_deref()).unwrap_or(failure.message),
            "param": null,
            "zenith_relay": {
                "origin": failure.origin.for_category(failure.category).as_str(),
                "category": failure.category,
                "request_id": request_id,
            },
        },
        "retry_at_ms": failure.retry_at_ms,
    })
}

pub(super) struct GatewayFailure {
    pub(super) upstream_error: Option<Box<crate::usage::UpstreamErrorDetails>>,
    pub(super) status: StatusCode,
    pub(super) category: &'static str,
    pub(super) message: &'static str,
    pub(super) retry_at_ms: Option<u64>,
    pub(super) origin: ErrorOrigin,
}

impl GatewayFailure {
    pub(super) fn with_upstream_error(
        mut self,
        details: Option<crate::usage::UpstreamErrorDetails>,
    ) -> Self {
        self.upstream_error = details.map(Box::new);
        self
    }

    pub(super) fn invalid_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: error_codes::INVALID_REQUEST,
            message,
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn continuation_unavailable() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            category: super::super::continuation::RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
            message: super::super::continuation::RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn model_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            category: error_codes::MODEL_NOT_FOUND,
            message: "model is not available in this managed pool",
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn request_timeout() -> Self {
        Self {
            status: StatusCode::REQUEST_TIMEOUT,
            category: error_codes::REQUEST_TIMEOUT,
            message: "response.create was not received in time",
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn client_closed() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: error_codes::CLIENT_CANCELLED,
            message: "client closed the WebSocket connection",
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn websocket_http_fallback(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::UPGRADE_REQUIRED,
            category: error_codes::UPSTREAM_WEBSOCKET_UNSUPPORTED,
            message: "upstream WebSocket is unavailable; using HTTP streaming",
            retry_at_ms: None,
            upstream_error: None,
            origin,
        }
    }

    pub(super) fn prepare(error: ExecutorPrepareError, origin: ErrorOrigin) -> Self {
        let failure = super::super::errors::AttemptFailure::prepare(error);
        Self {
            status: failure.status,
            category: failure.category,
            message: failure.message,
            retry_at_ms: None,
            upstream_error: None,
            origin,
        }
    }

    pub(super) fn transport(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: error_codes::UPSTREAM_TRANSPORT,
            message: "upstream WebSocket connection failed",
            retry_at_ms: None,
            upstream_error: None,
            origin,
        }
    }

    pub(super) fn closed(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: error_codes::UPSTREAM_WEBSOCKET_CLOSED,
            message: "upstream WebSocket closed before the response completed",
            retry_at_ms: None,
            upstream_error: None,
            origin,
        }
    }

    pub(super) fn message_too_large(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: error_codes::STREAM_EVENT_TOO_LARGE,
            message: "upstream WebSocket message exceeded the Relay size limit",
            retry_at_ms: None,
            upstream_error: None,
            origin,
        }
    }

    pub(super) fn upstream_status(
        status: StatusCode,
        body: Option<&[u8]>,
        origin: ErrorOrigin,
    ) -> Self {
        let classification = super::super::errors::classify_upstream_error(status, body);
        Self::classified(status, classification.category, origin).with_upstream_error(
            body.map(|body| {
                crate::usage::UpstreamErrorDetails::from_body(Some(status.as_u16()), body)
            }),
        )
    }

    pub(super) fn classified(
        status: StatusCode,
        category: &'static str,
        origin: ErrorOrigin,
    ) -> Self {
        Self {
            status: super::super::errors::canonical_upstream_status(status, category),
            category,
            message: super::super::errors::upstream_failure_message(category),
            retry_at_ms: None,
            upstream_error: None,
            origin,
        }
    }

    pub(super) fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: error_codes::NO_ELIGIBLE_SOURCE,
            message: "no eligible WebSocket source is available",
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn cooldown(retry_at_ms: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            category: error_codes::ALL_CANDIDATES_COOLING_DOWN,
            message: "all eligible sources are cooling down",
            retry_at_ms: Some(retry_at_ms),
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }
}

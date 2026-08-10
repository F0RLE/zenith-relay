use super::*;
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
            "message": failure.message,
            "param": null,
            "zenith_relay": {
                "origin": failure.origin.as_str(),
                "category": failure.category,
                "request_id": request_id,
            },
        },
        "retry_at_ms": failure.retry_at_ms,
    })
}

pub(super) struct GatewayFailure {
    pub(super) status: StatusCode,
    pub(super) category: &'static str,
    pub(super) message: &'static str,
    pub(super) retry_at_ms: Option<u64>,
    pub(super) origin: ErrorOrigin,
}

impl GatewayFailure {
    pub(super) fn invalid_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message,
            retry_at_ms: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn adapter_unsupported() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "adapter_websocket_not_supported",
            message: "the selected source adapter does not support Responses WebSocket transport",
            retry_at_ms: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn model_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            category: "model_not_found",
            message: "model is not available in this managed pool",
            retry_at_ms: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn request_timeout() -> Self {
        Self {
            status: StatusCode::REQUEST_TIMEOUT,
            category: "request_timeout",
            message: "response.create was not received in time",
            retry_at_ms: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn client_closed() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "client_cancelled",
            message: "client closed the WebSocket connection",
            retry_at_ms: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn prepare(error: ExecutorPrepareError, origin: ErrorOrigin) -> Self {
        let failure = super::super::errors::AttemptFailure::prepare(error);
        Self {
            status: failure.status,
            category: failure.category,
            message: failure.message,
            retry_at_ms: None,
            origin,
        }
    }

    pub(super) fn transport(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_transport",
            message: "upstream WebSocket connection failed",
            retry_at_ms: None,
            origin,
        }
    }

    pub(super) fn closed(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_websocket_closed",
            message: "upstream WebSocket closed before the response completed",
            retry_at_ms: None,
            origin,
        }
    }

    pub(super) fn idle_timeout(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            category: "websocket_idle_timeout",
            message: "upstream WebSocket produced no event before the idle timeout",
            retry_at_ms: None,
            origin,
        }
    }

    pub(super) fn semantic_timeout(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            category: "stream_semantic_timeout",
            message: "upstream produced no semantic output before the watchdog timeout",
            retry_at_ms: None,
            origin,
        }
    }

    pub(super) fn message_too_large(origin: ErrorOrigin) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "stream_event_too_large",
            message: "upstream WebSocket message exceeded the Relay size limit",
            retry_at_ms: None,
            origin,
        }
    }

    pub(super) fn upstream_status(
        status: StatusCode,
        body: Option<&[u8]>,
        origin: ErrorOrigin,
    ) -> Self {
        let classification = super::super::errors::classify_upstream_error(status, body);
        Self::classified(status, classification.category, origin)
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
            origin,
        }
    }

    pub(super) fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible WebSocket source is available",
            retry_at_ms: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn cooldown(retry_at_ms: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            category: "all_candidates_cooling_down",
            message: "all eligible sources are cooling down",
            retry_at_ms: Some(retry_at_ms),
            origin: ErrorOrigin::Relay,
        }
    }
}

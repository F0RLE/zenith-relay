use super::*;

pub(super) async fn send_gateway_error(downstream: &mut WebSocket, failure: &GatewayFailure) {
    let event = json!({
        "type": "error",
        "status": failure.status.as_u16(),
        "error": {
            "type": super::super::errors::api_error_type(
                failure.status,
                super::super::errors::api_error_code(failure.category),
            ),
            "code": super::super::errors::api_error_code(failure.category),
            "message": failure.message,
            "param": null,
        },
        "retry_at_ms": failure.retry_at_ms,
    });
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

pub(super) struct GatewayFailure {
    pub(super) status: StatusCode,
    pub(super) category: &'static str,
    pub(super) message: &'static str,
    pub(super) retry_at_ms: Option<u64>,
}

impl GatewayFailure {
    pub(super) fn invalid_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message,
            retry_at_ms: None,
        }
    }

    pub(super) fn adapter_unsupported() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "adapter_websocket_not_supported",
            message: "the selected source adapter does not support Responses WebSocket transport",
            retry_at_ms: None,
        }
    }

    pub(super) fn model_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            category: "model_not_found",
            message: "model is not available in this managed pool",
            retry_at_ms: None,
        }
    }

    pub(super) fn request_timeout() -> Self {
        Self {
            status: StatusCode::REQUEST_TIMEOUT,
            category: "request_timeout",
            message: "response.create was not received in time",
            retry_at_ms: None,
        }
    }

    pub(super) fn client_closed() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "client_cancelled",
            message: "client closed the WebSocket connection",
            retry_at_ms: None,
        }
    }

    pub(super) fn prepare(error: ExecutorPrepareError) -> Self {
        let failure = super::super::errors::AttemptFailure::prepare(error);
        Self {
            status: failure.status,
            category: failure.category,
            message: failure.message,
            retry_at_ms: None,
        }
    }

    pub(super) fn transport() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_transport",
            message: "upstream WebSocket connection failed",
            retry_at_ms: None,
        }
    }

    pub(super) fn closed() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_websocket_closed",
            message: "upstream WebSocket closed before the response completed",
            retry_at_ms: None,
        }
    }

    pub(super) fn idle_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            category: "websocket_idle_timeout",
            message: "upstream WebSocket produced no event before the idle timeout",
            retry_at_ms: None,
        }
    }

    pub(super) fn semantic_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            category: "stream_semantic_timeout",
            message: "upstream produced no semantic output before the watchdog timeout",
            retry_at_ms: None,
        }
    }

    pub(super) fn bootstrap_too_large() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "stream_event_too_large",
            message: "upstream WebSocket bootstrap is too large",
            retry_at_ms: None,
        }
    }

    pub(super) fn upstream_status(status: StatusCode, body: Option<&[u8]>) -> Self {
        let classification = super::super::errors::classify_upstream_error(status, body);
        Self::classified(status, classification.category)
    }

    pub(super) fn classified(status: StatusCode, category: &'static str) -> Self {
        Self {
            status: super::super::errors::canonical_upstream_status(status, category),
            category,
            message: super::super::errors::upstream_failure_message(category),
            retry_at_ms: None,
        }
    }

    pub(super) fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible WebSocket source is available",
            retry_at_ms: None,
        }
    }

    pub(super) fn cooldown(retry_at_ms: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            category: "all_candidates_cooling_down",
            message: "all eligible sources are cooling down",
            retry_at_ms: Some(retry_at_ms),
        }
    }
}

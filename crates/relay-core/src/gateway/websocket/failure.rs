use super::*;
use crate::ErrorOrigin;
use crate::error_codes;

pub(super) async fn send_gateway_error(
    downstream: &mut WebSocket,
    failure: &GatewayFailure,
    request_id: Option<&str>,
    stream_id: Option<&str>,
) {
    let event = gateway_error_event(failure, request_id, stream_id);
    if send_message(downstream, Message::Text(event.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    let _ = send_message(
        downstream,
        Message::Close(Some(CloseFrame {
            code: if failure.status.is_client_error() {
                close_code::POLICY
            } else {
                close_code::ERROR
            },
            reason: "request failed".into(),
        })),
    )
    .await;
}

pub(super) fn gateway_error_event(
    failure: &GatewayFailure,
    request_id: Option<&str>,
    stream_id: Option<&str>,
) -> Value {
    // Responses clients recognize this code and can resend their complete
    // history after reconnecting. Keep the Relay category in diagnostics and
    // never turn an ownership mismatch into permission to reset a conversation.
    let local_history_missing = failure.origin == ErrorOrigin::Relay
        && failure.category == error_codes::ADAPTER_CONTINUATION_MISSING;
    let code = super::super::errors::api_error_code(if local_history_missing {
        error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
    } else {
        failure.category
    });
    let origin = failure.origin.for_category(failure.category);
    let message = failure
        .upstream_error
        .as_ref()
        .and_then(|details| details.message.as_deref())
        .unwrap_or(failure.message);
    let mut event = json!({
        "type": "error",
        "status": failure.status.as_u16(),
        "error": {
            "type": super::super::errors::api_error_type(
                failure.status,
                code,
            ),
            "code": code,
            "message": origin.prefix_message(message),
            "param": local_history_missing.then_some("previous_response_id"),
            "zenith_relay": {
                "origin": origin.as_str(),
                "category": failure.category,
                "request_id": request_id,
            },
        },
        "retry_at_ms": failure.retry_at_ms,
    });
    if let Some(stream_id) = stream_id {
        event["stream_id"] = json!(stream_id);
    }
    event
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
    pub(super) fn blocked_memory(kind: &str) -> Option<Self> {
        let category = match kind {
            "memory_extraction" => error_codes::CODEX_BACKGROUND_BLOCKED_MEMORY_EXTRACTION,
            "memory_consolidation" => error_codes::CODEX_BACKGROUND_BLOCKED_MEMORY_CONSOLIDATION,
            _ => return None,
        };
        Some(Self {
            upstream_error: None,
            status: StatusCode::FORBIDDEN,
            category,
            message: "This memory stage is disabled in Relay",
            retry_at_ms: None,
            origin: ErrorOrigin::Relay,
        })
    }

    pub(super) fn admission(reason: crate::scheduler::rotation::AdmissionStopReason) -> Self {
        let (category, message) = super::super::errors::admission_failure(reason);
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category,
            message,
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn with_upstream_error(
        mut self,
        details: Option<crate::usage::UpstreamErrorDetails>,
    ) -> Self {
        self.upstream_error = details.map(Box::new);
        self
    }

    pub(super) fn with_retry_headers(mut self, headers: &HeaderMap) -> Self {
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            let retry_at = crate::transport::retry_after_ms(headers, std::time::SystemTime::now())
                .map(|delay| now_ms().saturating_add(delay));
            self.retry_at_ms = self.retry_at_ms.into_iter().chain(retry_at).max();
        }
        self
    }

    pub(super) fn apply_degraded_route_policy(mut self, runtime: &GatewayRuntime) -> Self {
        let category = runtime.effective_upstream_category(self.category);
        if category != self.category {
            self.status = super::super::errors::canonical_upstream_status(self.status, category);
            self.message = super::super::errors::upstream_failure_message(category);
            self.category = category;
        }
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

    pub(super) fn unsupported_service_tier() -> Self {
        Self {
            category: error_codes::ADAPTER_PARAMETER_UNSUPPORTED,
            ..Self::invalid_request("service_tier is not supported by the configured routes")
        }
    }

    pub(super) fn adapter(error: crate::AdapterError) -> Self {
        Self {
            status: if error.is_upstream_failure() {
                StatusCode::BAD_GATEWAY
            } else {
                StatusCode::BAD_REQUEST
            },
            category: error.code(),
            message: error.message(),
            retry_at_ms: None,
            upstream_error: None,
            origin: ErrorOrigin::Relay,
        }
    }

    pub(super) fn invalid_stream_id() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: error_codes::INVALID_STREAM_ID,
            message: "stream_id must be 1-256 ASCII letters, digits, underscores, hyphens or periods",
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

    pub(super) fn websocket_slow_handshake(origin: ErrorOrigin) -> Self {
        Self {
            category: error_codes::UPSTREAM_WEBSOCKET_SLOW_HANDSHAKE,
            message: "optional WebSocket handshake exceeded its budget; using HTTP streaming",
            ..Self::websocket_http_fallback(origin)
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
        upstream_error_body: Option<&[u8]>,
        origin: ErrorOrigin,
    ) -> Self {
        let classification =
            super::super::errors::classify_upstream_error(status, upstream_error_body);
        let mut failure = Self::classified(status, classification.category, origin)
            .with_upstream_error(upstream_error_body.map(|error_body| {
                crate::usage::UpstreamErrorDetails::from_response_body(
                    Some(status.as_u16()),
                    error_body,
                )
            }));
        if failure.status == StatusCode::TOO_MANY_REQUESTS {
            failure.retry_at_ms = upstream_error_body
                .map(rate_limit_body_hint)
                .and_then(|hint| hint.retry_after_for_account(origin == ErrorOrigin::Account))
                .map(|delay| now_ms().saturating_add(delay));
        }
        failure
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

use super::*;

impl AttemptFailure {
    pub(crate) fn authorized_request(error: AuthorizedRequestError) -> Self {
        match error {
            AuthorizedRequestError::Prepare(error) => Self::prepare(error),
            AuthorizedRequestError::Transport(error) => Self::transport(&error),
            AuthorizedRequestError::NotReplayable => Self::body(),
        }
    }

    pub(crate) fn transport(error: &reqwest::Error) -> Self {
        let (category, message) = if error.is_timeout() {
            ("upstream_transport_timeout", "upstream request timed out")
        } else if error.is_connect() {
            (
                "upstream_transport_connect",
                "upstream connection could not be established",
            )
        } else if error.is_body() {
            (
                "upstream_transport_body",
                "upstream request or response body failed",
            )
        } else if error.is_request() {
            ("upstream_transport_request", "upstream request failed")
        } else {
            ("upstream_transport", "upstream transport failed")
        };
        Self {
            status: StatusCode::BAD_GATEWAY,
            category,
            message,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn body() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_error",
            message: "upstream response failed",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn invalid_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message: "request cannot be translated for an eligible source",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn status_with_body(status: StatusCode, body: Option<&[u8]>) -> Self {
        let classification = classify_upstream_error(status, body);
        Self {
            status: canonical_upstream_status(status, classification.category),
            category: classification.category,
            message: classification.message,
            cooldown_hint: body.map(rate_limit_body_hint).unwrap_or_default(),
        }
    }

    pub(crate) fn classified_with_hint(
        status: StatusCode,
        category: &'static str,
        cooldown_hint: RateLimitBodyHint,
    ) -> Self {
        Self {
            status: canonical_upstream_status(status, category),
            category,
            message: upstream_failure_message(category),
            cooldown_hint,
        }
    }

    pub(crate) fn stream(category: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category,
            message: "upstream stream failed before client output",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn no_candidate() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible source is available for this model",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn prepare(error: ExecutorPrepareError) -> Self {
        match error {
            ExecutorPrepareError::Authentication | ExecutorPrepareError::InvalidCredential => {
                Self {
                    status: StatusCode::UNAUTHORIZED,
                    category: "account_auth",
                    message: "account authorization is unavailable",
                    cooldown_hint: RateLimitBodyHint::default(),
                }
            }
            ExecutorPrepareError::Persistence => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                category: "account_token_persistence",
                message: "refreshed account authorization could not be persisted",
                cooldown_hint: RateLimitBodyHint::default(),
            },
            ExecutorPrepareError::Transient => Self {
                status: StatusCode::BAD_GATEWAY,
                category: "account_refresh",
                message: "account authorization refresh failed",
                cooldown_hint: RateLimitBodyHint::default(),
            },
        }
    }
}

pub(crate) fn retryable_status(status: StatusCode, has_previous_response_id: bool) -> bool {
    matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::FORBIDDEN
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::CONFLICT
            | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
        || (status == StatusCode::NOT_FOUND && !has_previous_response_id)
}

pub(crate) fn retryable_failure(
    status: StatusCode,
    category: &str,
    has_previous_response_id: bool,
) -> bool {
    if !failure_category_requires_cooldown(category) {
        return false;
    }
    if matches!(
        category,
        "upstream_unauthorized"
            | "upstream_account_disabled"
            | "upstream_forbidden"
            | "upstream_region_unsupported"
            | "upstream_model_not_found"
            | "upstream_content_policy"
            | "upstream_invalid_request"
    ) {
        return false;
    }
    if category == "upstream_candidate_rejected" {
        // A new request may safely try another source. Responses continuations
        // are bound to their creator, so retrying an unclassified rejection
        // elsewhere can corrupt its upstream conversation state.
        return !has_previous_response_id;
    }
    retryable_status(status, has_previous_response_id)
        || matches!(
            category,
            "upstream_usage_not_included"
                | "upstream_quota_exhausted"
                | "upstream_model_unsupported"
                | "upstream_model_capacity"
                | "upstream_websocket_connection_limit"
                | "upstream_rate_limited"
                | "upstream_refresh_token_reused"
                | "upstream_request_timeout"
                | "upstream_overloaded"
                | "upstream_edge_challenge"
                | "upstream_server_error"
                | "upstream_bad_gateway"
                | "upstream_unavailable"
                | "upstream_gateway_timeout"
                | "upstream_transport_timeout"
                | "upstream_transport_connect"
                | "upstream_transport_body"
                | "upstream_transport_request"
                | "upstream_transport"
                | "upstream_error"
        )
}

pub(crate) fn failure_category_requires_cooldown(category: &str) -> bool {
    !matches!(
        category,
        "client_cancelled"
            | "response_affinity_miss"
            | "response_incomplete"
            | "upstream_cancelled"
            | "upstream_previous_response_not_found"
            | "upstream_tool_call_mismatch"
            | "upstream_context_too_large"
            | "upstream_encrypted_content_invalid"
            | "upstream_instructions_required"
            | "upstream_content_policy"
            | "upstream_payload_too_large"
            | "upstream_unsupported_request"
            | "upstream_websocket_unsupported"
            | "upstream_invalid_request"
    )
}

pub(crate) fn failure_category_is_request_terminal(category: &str) -> bool {
    matches!(
        category,
        "upstream_tool_call_mismatch"
            | "upstream_context_too_large"
            | "upstream_encrypted_content_invalid"
            | "upstream_instructions_required"
            | "upstream_content_policy"
            | "upstream_payload_too_large"
            | "upstream_unsupported_request"
            | "upstream_websocket_unsupported"
            | "upstream_invalid_request"
    )
}

pub(crate) fn recoverable_response_affinity_miss(
    status: StatusCode,
    has_previous_response_id: bool,
    _response_affinity_hit: bool,
    previous_response_not_found: bool,
) -> bool {
    has_previous_response_id
        && previous_response_not_found
        && matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::CONFLICT
        )
}

pub(crate) fn retry_candidate_limit(
    max_retry_candidates: usize,
    owner_recovery_confirmed: bool,
) -> usize {
    if owner_recovery_confirmed {
        MAX_RESPONSE_OWNER_CANDIDATES
    } else {
        max_retry_candidates
    }
}

pub(crate) fn previous_response_not_found(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .ok()
        .is_some_and(|value| previous_response_not_found_value(&value))
}

pub(crate) fn previous_response_requires_websocket(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    let text = serde_json::to_string(&value)
        .unwrap_or_default()
        .to_ascii_lowercase();
    text.contains("previous_response_id") && text.contains("websocket")
}

pub(crate) fn responses_function_call_output_has_invalid_call_id(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    let text = upstream_error_text(&value);
    text_has_any(
        &text,
        &[
            "invalid call_id for function_call_output",
            "invalid call id for function_call_output",
            "invalid_call_id_for_function_call_output",
            "invalid_function_call_output_call_id",
        ],
    )
}

/// Detects the narrow Responses validation failure caused by a historical
/// tool item that omitted `call_id`. This deliberately does not match generic
/// invalid-call-id or arbitrary required-field errors: the request repair is
/// allowed only when the upstream identifies the missing field itself.
pub(crate) fn responses_call_id_is_missing(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return responses_call_id_is_missing_text(&normalized_error_text(payload));
    };
    responses_call_id_is_missing_value(&value)
}

pub(crate) fn responses_call_id_is_missing_value(value: &Value) -> bool {
    let code = [
        "/code",
        "/error/code",
        "/body/error/code",
        "/response/error/code",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .any(|code| {
        matches!(
            code.to_ascii_lowercase().as_str(),
            "missing_call_id" | "call_id_required" | "missing_required_call_id"
        )
    });
    code || responses_call_id_is_missing_text(&upstream_error_text(value))
}

fn responses_call_id_is_missing_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    if (!text.contains("call_id") && !text.contains("call id"))
        || text.contains("invalid call_id")
        || text.contains("invalid call id")
    {
        return false;
    }
    let normalized = text
        .chars()
        .map(|character| match character {
            '`' | '\'' | '"' | ':' | '.' | '-' | '/' => ' ',
            character => character,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    text_has_any(
        &normalized,
        &[
            "missing field call_id",
            "missing required field call_id",
            "missing required parameter call_id",
            "missing parameter call_id",
            "required field call_id",
            "required parameter call_id",
            "field call_id is required",
            "parameter call_id is required",
            "call_id is required",
            "call_id was required",
            "call_id is missing",
            "call_id was missing",
            "missing call_id",
            "call_id missing",
            "call_id must be provided",
            "call_id must be present",
            "call_id was not provided",
            "missing field call id",
            "missing required field call id",
            "missing required parameter call id",
            "missing parameter call id",
            "required field call id",
            "required parameter call id",
            "field call id is required",
            "parameter call id is required",
            "call id is required",
            "call id was required",
            "call id is missing",
            "call id was missing",
            "missing call id",
            "call id missing",
            "call id must be provided",
            "call id must be present",
            "call id was not provided",
        ],
    )
}

/// Detects the specific Responses rejection produced when imported history
/// contains a tool call without its matching output. The caller must still
/// prove that the request contains such an incomplete call before recovery.
pub(crate) fn responses_tool_call_is_missing_output(payload: &[u8]) -> bool {
    responses_tool_call_is_missing_output_message(&normalized_error_text(payload))
}

pub(crate) fn responses_tool_call_is_missing_output_message(message: &str) -> bool {
    text_has_any(
        &message.to_ascii_lowercase(),
        &[
            "no tool output found for function call",
            "no tool output found for custom tool call",
            "no tool output found for apply patch call",
            "unanswered_function_call",
        ],
    )
}

/// Zenith Gateway intentionally hides provider-specific 400 details. A
/// Responses continuation with tool output can use the local replay state to
/// recover the preceding tool call when this exact public envelope is returned.
/// Do not match arbitrary 400 responses: those may be genuine client errors.
pub(crate) fn zenith_gateway_invalid_request(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    zenith_gateway_invalid_request_value(&value)
}

pub(crate) fn zenith_gateway_invalid_request_value(value: &Value) -> bool {
    const MESSAGE: &str =
        "Zenith AI request is invalid. Check the model, messages, tools, and parameters.";

    [
        "/error/message",
        "/response/error/message",
        "/body/error/message",
        "/message",
        "/response/message",
        "/body/message",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .any(|message| message.trim().eq_ignore_ascii_case(MESSAGE))
}

/// Strict Responses endpoints use a separate `fc_` namespace for
/// `function_call.id`; the matching `call_id` is unchanged. This is only a
/// recovery signal — the request repair itself still verifies that it has a
/// call-prefixed function item before retrying.
pub(crate) fn responses_function_item_id_requires_fc_prefix(payload: &[u8]) -> bool {
    let text = normalized_error_text(payload);
    text.contains("input") && text.contains("expected an id that begins with 'fc'")
}

/// Strict Responses endpoints use `ctc_` for `custom_tool_call.id`.
pub(crate) fn responses_custom_tool_item_id_requires_ctc_prefix(payload: &[u8]) -> bool {
    let text = normalized_error_text(payload);
    text.contains("input")
        && text.contains(".id")
        && (text.contains("expected an id that begins with 'ctc'")
            || text.contains("expected an id that begins with 'ctc_'")
            || text.contains("expected an id that starts with 'ctc'"))
}

/// Strict Responses endpoints require server-owned `msg_` item identifiers on
/// message inputs. This only identifies the precise upstream validation error;
/// the repair still verifies the foreign `item_` identifier before retrying.
pub(crate) fn responses_message_item_id_requires_msg_prefix(payload: &[u8]) -> bool {
    let text = normalized_error_text(payload);
    text.contains("input[")
        && text.contains(".id")
        && text.contains("expected an id that begins with 'msg'")
        // Some Responses-compatible gateways report stale message items
        // after context compaction as a missing text part instead.
        || text.contains("text part msg_") && text.contains(" not found")
}

/// A client can deliberately switch models in a Responses conversation.  The
/// old upstream response is then unusable by the new route, but a normal text
/// continuation is still replayable without `previous_response_id`.  Never
/// apply this escape hatch to an orphaned tool output: without its matching
/// function-call item, sending it to another route would corrupt tool state.
pub(crate) fn recoverable_response_model_switch(
    status: StatusCode,
    category: &str,
    has_previous_response_id: bool,
    has_unpaired_tool_output: bool,
    payload: &[u8],
) -> bool {
    if status != StatusCode::BAD_REQUEST || !has_previous_response_id || has_unpaired_tool_output {
        return false;
    }

    category == "upstream_tool_call_mismatch" || {
        let text = normalized_error_text(payload);
        (text.contains("previous_response_id")
            && text_has_any(&text, &["model", "mismatch", "switch"]))
            || (text.contains("previous response") && text_has_any(&text, &["model", "mismatch"]))
            || (text.contains("tool") && text_has_any(&text, &["model", "mismatch"]))
    }
}

/// A cache write is a route-local optimization. Providers use several
/// different error envelopes for rejecting cache-control/ephemeral writes;
/// treat only explicit cache-write wording as a retryable route failure.
pub(crate) fn prompt_cache_write_rejected(payload: &[u8]) -> bool {
    let text = normalized_error_text(payload);
    text.contains("cache")
        && text_has_any(
            &text,
            &[
                "write",
                "writ",
                "creation",
                "create",
                "cache_control",
                "cache-control",
                "ephemeral",
                "ttl",
            ],
        )
}

pub(crate) fn previous_response_not_found_value(value: &Value) -> bool {
    [value.pointer("/error/code"), value.pointer("/error/type")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("previous_response_not_found")
                || value
                    .trim()
                    .eq_ignore_ascii_case("response_continuation_unavailable")
        })
        || [value.pointer("/error/message"), value.get("message")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(previous_response_not_found_message)
}

fn previous_response_not_found_message(message: &str) -> bool {
    let message = message.trim().trim_end_matches('.').to_ascii_lowercase();
    message == "previous response not found"
        || (message.starts_with("previous response with id ") && message.ends_with(" not found"))
        || message.starts_with("no response found for previous_response_id ")
}

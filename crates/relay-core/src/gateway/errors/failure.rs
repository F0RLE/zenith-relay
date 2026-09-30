use super::*;
use crate::error_codes;
use crate::scheduler::rotation::{AttemptObservation, ExecutionObservation, HealthObservation};

mod response;

#[cfg(test)]
pub(super) use response::responses_call_id_is_missing;
pub(super) use response::responses_call_id_is_missing_text;
pub(crate) use response::{
    previous_response_not_found, previous_response_not_found_value,
    previous_response_requires_websocket, prompt_cache_write_rejected,
    recoverable_response_affinity_miss, recoverable_response_model_switch,
    responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_call_output_has_invalid_call_id,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    responses_tool_call_is_missing_output, responses_tool_call_is_missing_output_message,
    responses_tool_call_links_rejected, responses_tool_call_links_rejected_value,
    zenith_gateway_invalid_request, zenith_gateway_invalid_request_value,
};

impl AttemptFailure {
    /// Explicit provider rejections and connection failures have different
    /// health effects from authentication, allowance and local preparation.
    pub(crate) fn settle_rotation_rejection(
        &self,
        runtime: &GatewayRuntime,
        lease: &crate::runtime::CandidateLease,
        cooldown: Option<CooldownRequest<'_>>,
        now_ms: u64,
    ) {
        let health = if !lease.has_dispatched()
            || matches!(
                self.category,
                error_codes::ACCOUNT_REFRESH
                    | error_codes::ACCOUNT_TOKEN_PERSISTENCE
                    | error_codes::ACCOUNT_AUTH
            ) {
            HealthObservation::LocalError
        } else if matches!(
            self.category,
            error_codes::UPSTREAM_SERVER_ERROR
                | error_codes::UPSTREAM_BAD_GATEWAY
                | error_codes::UPSTREAM_UNAVAILABLE
                | error_codes::UPSTREAM_GATEWAY_TIMEOUT
                | error_codes::UPSTREAM_OVERLOADED
                | error_codes::UPSTREAM_TRANSPORT_CONNECT
        ) {
            HealthObservation::CountableTransient {
                provider_not_before_ms: cooldown.map(|request| request.retry_at_ms).or_else(|| {
                    self.cooldown_hint
                        .retry_after_ms
                        .map(|delay| now_ms.saturating_add(delay))
                }),
            }
        } else if self.execution.certainty
            == crate::scheduler::rotation::ExecutionCertainty::Unknown
        {
            HealthObservation::Unknown
        } else {
            HealthObservation::ClientError
        };
        runtime.settle_rotation_failure(
            lease,
            AttemptObservation {
                execution: self.execution,
                health,
            },
            cooldown,
            now_ms,
        );
    }

    pub(crate) fn authorized_request(error: AuthorizedRequestError) -> Self {
        match error {
            AuthorizedRequestError::Prepare(error) => Self::prepare(error),
            AuthorizedRequestError::Transport(error) => Self::transport(&error),
            AuthorizedRequestError::NotReplayable => Self::body(),
            AuthorizedRequestError::DispatchBudgetExhausted => Self::no_candidate(),
        }
    }

    pub(crate) fn transport(error: &reqwest::Error) -> Self {
        let (category, message) = if error.is_timeout() {
            (
                error_codes::UPSTREAM_TRANSPORT_TIMEOUT,
                "upstream request timed out",
            )
        } else if error.is_connect() {
            (
                error_codes::UPSTREAM_TRANSPORT_CONNECT,
                "upstream connection could not be established",
            )
        } else if error.is_body() {
            (
                error_codes::UPSTREAM_TRANSPORT_BODY,
                "upstream request or response body failed",
            )
        } else if error.is_request() {
            (
                error_codes::UPSTREAM_TRANSPORT_REQUEST,
                "upstream request failed",
            )
        } else {
            (error_codes::UPSTREAM_TRANSPORT, "upstream transport failed")
        };
        Self {
            execution: if error.is_connect() {
                ExecutionObservation::not_sent()
            } else {
                ExecutionObservation::unknown()
            },
            status: StatusCode::BAD_GATEWAY,
            category,
            message,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn body() -> Self {
        Self {
            execution: ExecutionObservation::unknown(),
            status: StatusCode::BAD_GATEWAY,
            category: error_codes::UPSTREAM_ERROR,
            message: "upstream response failed",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn invalid_request() -> Self {
        Self {
            execution: ExecutionObservation::not_sent(),
            status: StatusCode::BAD_REQUEST,
            category: error_codes::INVALID_REQUEST,
            message: "request cannot be translated for an eligible source",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn status_with_body(status: StatusCode, body: Option<&[u8]>) -> Self {
        let classification = classify_upstream_error(status, body);
        Self {
            execution: rejection_execution(status, classification.category),
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
            execution: rejection_execution(status, category),
            status: canonical_upstream_status(status, category),
            category,
            message: upstream_failure_message(category),
            cooldown_hint,
        }
    }

    pub(crate) fn stream(category: &'static str) -> Self {
        Self {
            execution: ExecutionObservation::unknown(),
            status: StatusCode::BAD_GATEWAY,
            category,
            message: "upstream stream failed before client output",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(crate) fn no_candidate() -> Self {
        Self {
            execution: ExecutionObservation::not_sent(),
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: error_codes::NO_ELIGIBLE_SOURCE,
            message: "no eligible source is available for this model",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    /// A closed retry window is an upstream availability failure.
    /// An open window keeps the last rejection, or reports that nothing was eligible.
    pub(crate) fn after_exhausted_attempts(
        retry_window_expired: bool,
        last_failure: Option<Self>,
    ) -> Self {
        if retry_window_expired {
            Self::classified_with_hint(
                StatusCode::SERVICE_UNAVAILABLE,
                error_codes::UPSTREAM_UNAVAILABLE,
                Default::default(),
            )
        } else {
            last_failure.unwrap_or_else(Self::no_candidate)
        }
    }

    pub(crate) fn prepare(error: ExecutorPrepareError) -> Self {
        match error {
            ExecutorPrepareError::Authentication | ExecutorPrepareError::InvalidCredential => {
                Self {
                    execution: ExecutionObservation::not_sent(),
                    status: StatusCode::UNAUTHORIZED,
                    category: error_codes::ACCOUNT_AUTH,
                    message: "account authorization is unavailable",
                    cooldown_hint: RateLimitBodyHint::default(),
                }
            }
            ExecutorPrepareError::Persistence => Self {
                execution: ExecutionObservation::not_sent(),
                status: StatusCode::SERVICE_UNAVAILABLE,
                category: error_codes::ACCOUNT_TOKEN_PERSISTENCE,
                message: "refreshed account authorization could not be persisted",
                cooldown_hint: RateLimitBodyHint::default(),
            },
            ExecutorPrepareError::Transient => Self {
                execution: ExecutionObservation::not_sent(),
                status: StatusCode::BAD_GATEWAY,
                category: error_codes::ACCOUNT_REFRESH,
                message: "account authorization refresh failed",
                cooldown_hint: RateLimitBodyHint::default(),
            },
        }
    }
}

/// HTTP status alone does not prove that a non-idempotent operation was
/// rejected before execution. Only admission failures can authorize replay;
/// an unclassified terminal event, timeout or generic 5xx remains uncertain.
fn rejection_execution(status: StatusCode, category: &str) -> ExecutionObservation {
    if matches!(
        category,
        error_codes::UPSTREAM_TERMINAL
            | error_codes::UPSTREAM_REQUEST_TIMEOUT
            | error_codes::UPSTREAM_CONFLICT
            | error_codes::UPSTREAM_TRANSPORT
            | error_codes::UPSTREAM_GATEWAY_TIMEOUT
    ) {
        return ExecutionObservation::unknown();
    }
    if status.is_client_error()
        || matches!(
            category,
            error_codes::UPSTREAM_OVERLOADED
                | error_codes::UPSTREAM_MODEL_CAPACITY
                | error_codes::UPSTREAM_RATE_LIMITED
                | error_codes::UPSTREAM_QUOTA_EXHAUSTED
                | error_codes::UPSTREAM_WEBSOCKET_CONNECTION_LIMIT
                | error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
                | error_codes::UPSTREAM_CANDIDATE_REJECTED
        )
    {
        ExecutionObservation::not_sent()
    } else {
        ExecutionObservation::unknown()
    }
}

mod policy;

pub(crate) use policy::{
    failure_category_affects_account_state, failure_category_is_request_terminal,
    failure_category_requires_cooldown, retryable_failure, retryable_status,
};

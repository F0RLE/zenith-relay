use crate::error_codes;
use std::fmt;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterError {
    code: &'static str,
    message: &'static str,
    parameter: Option<&'static str>,
}

impl AdapterError {
    pub const fn code(self) -> &'static str {
        self.code
    }

    pub const fn message(self) -> &'static str {
        self.message
    }

    pub const fn parameter(self) -> Option<&'static str> {
        self.parameter
    }

    pub(crate) const fn with_parameter(mut self, parameter: &'static str) -> Self {
        self.parameter = Some(parameter);
        self
    }

    pub(crate) const fn parameter_unsupported_for(parameter: &'static str) -> Self {
        Self::parameter_unsupported().with_parameter(parameter)
    }

    pub fn is_upstream_failure(self) -> bool {
        matches!(
            self.code,
            error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID
                | error_codes::ADAPTER_UPSTREAM_STREAM_INVALID
        )
    }

    pub(crate) fn is_route_incompatible(self) -> bool {
        matches!(
            self.code,
            error_codes::ADAPTER_TOOL_UNSUPPORTED
                | error_codes::ADAPTER_PARAMETER_UNSUPPORTED
                | error_codes::ADAPTER_BINDING_UNSUPPORTED
                | error_codes::ADAPTER_REASONING_UNSUPPORTED
                | error_codes::ADAPTER_COMPACTION_UNSUPPORTED
        )
    }

    pub(crate) const fn invalid_request() -> Self {
        Self {
            code: error_codes::ADAPTER_INVALID_REQUEST,
            message: "request cannot be represented by the selected source adapter",
            parameter: None,
        }
    }

    pub(in crate::protocol::adapter) const fn compaction_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_COMPACTION_UNSUPPORTED,
            message: "Responses compaction history requires a native Responses route",
            parameter: Some("input"),
        }
    }

    pub(crate) const fn parameter_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_PARAMETER_UNSUPPORTED,
            message: "a request parameter has no lossless mapping on this route; use a compatible native endpoint",
            parameter: None,
        }
    }

    pub(in crate::protocol::adapter) const fn continuation_missing() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISSING,
            message: "the adapter no longer has the prior response needed for this continuation",
            parameter: None,
        }
    }

    pub(in crate::protocol::adapter) const fn continuation_mismatch() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISMATCH,
            message: "the continuation belongs to a different model or source route",
            parameter: None,
        }
    }

    pub(crate) const fn unsupported_binding() -> Self {
        Self {
            code: error_codes::ADAPTER_BINDING_UNSUPPORTED,
            message: "the selected adapter cannot serve this client protocol",
            parameter: None,
        }
    }

    pub(in crate::protocol::adapter) const fn unsupported_tool() -> Self {
        Self {
            code: error_codes::ADAPTER_TOOL_UNSUPPORTED,
            message: "the selected source adapter supports JSON-schema function and direct custom text tools only",
            parameter: None,
        }
    }

    pub(crate) const fn reasoning_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_REASONING_UNSUPPORTED,
            message: "the selected source adapter does not expose reasoning for this binding",
            parameter: None,
        }
    }

    pub(crate) const fn upstream_response_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "the upstream response cannot be represented as a Responses response",
            parameter: None,
        }
    }

    pub(crate) const fn upstream_stream_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
            message: "the upstream stream cannot be represented as a Responses stream",
            parameter: None,
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for AdapterError {}

pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

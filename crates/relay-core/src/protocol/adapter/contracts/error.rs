use crate::error_codes;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct JsonParseDiagnostic {
    reason: &'static str,
    shape: &'static str,
    line: usize,
    column: usize,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterError {
    code: &'static str,
    message: &'static str,
    parameter: Option<&'static str>,
    json_diagnostic: Option<JsonParseDiagnostic>,
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

    pub(crate) const fn with_json_diagnostic(
        mut self,
        reason: &'static str,
        shape: &'static str,
        line: usize,
        column: usize,
        bytes: usize,
    ) -> Self {
        self.json_diagnostic = Some(JsonParseDiagnostic {
            reason,
            shape,
            line,
            column,
            bytes,
        });
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
            json_diagnostic: None,
        }
    }

    pub(in crate::protocol::adapter) const fn compaction_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_COMPACTION_UNSUPPORTED,
            message: "Responses compaction history requires a native Responses route",
            parameter: Some("input"),
            json_diagnostic: None,
        }
    }

    pub(crate) const fn parameter_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_PARAMETER_UNSUPPORTED,
            message: "a request parameter has no lossless mapping on this route; use a compatible native endpoint",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(crate) const fn continuation_missing() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISSING,
            message: "the adapter no longer has the prior response needed for this continuation",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(crate) const fn continuation_mismatch() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISMATCH,
            message: "the continuation belongs to a different model or source route",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(crate) const fn unsupported_binding() -> Self {
        Self {
            code: error_codes::ADAPTER_BINDING_UNSUPPORTED,
            message: "the selected adapter cannot serve this client protocol",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(in crate::protocol::adapter) const fn unsupported_tool() -> Self {
        Self {
            code: error_codes::ADAPTER_TOOL_UNSUPPORTED,
            message:
                "the selected source adapter cannot represent the requested tool type or format",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(crate) const fn reasoning_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_REASONING_UNSUPPORTED,
            message: "the selected source adapter does not expose reasoning for this binding",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(crate) const fn upstream_response_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "the upstream response cannot be represented by the selected client protocol",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(crate) const fn upstream_tool_arguments_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "tool call arguments do not match the declared JSON Schema",
            parameter: Some("output.run_officejs.args.schema"),
            json_diagnostic: None,
        }
    }

    pub(crate) const fn upstream_tool_arguments_json_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "function tool arguments must be one complete JSON object; for script or command strings use the explicit relay-function-string-v1 frame instead of nested JSON escaping",
            parameter: None,
            json_diagnostic: None,
        }
    }

    pub(crate) const fn upstream_tool_arguments_json_incomplete() -> Self {
        Self {
            message: "function tool arguments must be one complete JSON object; the inner JSON ended before the object or string was complete; no client tool was released",
            ..Self::upstream_tool_arguments_json_invalid()
        }
    }

    pub(crate) const fn upstream_tool_arguments_json_syntax_invalid() -> Self {
        Self {
            message: "function tool arguments must be one complete JSON object; the inner JSON has invalid syntax, escaping, or extra code; use the explicit relay-function-string-v1 frame for script strings",
            ..Self::upstream_tool_arguments_json_invalid()
        }
    }

    pub(crate) const fn upstream_tool_arguments_frame_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "function string arguments need one frame header with field and args, followed by a newline and the exact string value; do not repeat the field in args",
            parameter: Some("output.run_officejs.args.frame"),
            json_diagnostic: None,
        }
    }

    pub(crate) const fn upstream_stream_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
            message: "the upstream stream is incomplete, invalid, or cannot be represented by the selected client protocol",
            parameter: None,
            json_diagnostic: None,
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)?;
        if let Some(diagnostic) = self.json_diagnostic {
            write!(formatter, " (json_reason={}, payload_shape={}, parse_line={}, parse_column={}, payload_bytes={})",
                diagnostic.reason, diagnostic.shape, diagnostic.line, diagnostic.column, diagnostic.bytes)?;
        }
        Ok(())
    }
}

impl std::error::Error for AdapterError {}

pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

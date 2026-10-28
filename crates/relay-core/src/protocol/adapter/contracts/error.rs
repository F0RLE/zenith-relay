use crate::error_codes;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShapeDiagnostic {
    shape: &'static str,
    fields: usize,
    rejected_field: &'static str,
    rejected_shape: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct JsonParseDiagnostic {
    reason: &'static str,
    shape: &'static str,
    line: usize,
    column: usize,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SchemaDiagnostic {
    phase: &'static str,
    keyword: &'static str,
    schema_depth: usize,
    instance_depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdapterDiagnostic {
    Json(JsonParseDiagnostic),
    Shape(ShapeDiagnostic),
    Schema(SchemaDiagnostic),
    ToolIdentity(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterError {
    code: &'static str,
    message: &'static str,
    parameter: Option<&'static str>,
    diagnostic: Option<AdapterDiagnostic>,
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

    pub(crate) fn with_shape_diagnostic(
        mut self,
        value: &serde_json::Value,
        rejected: Option<(&'static str, &serde_json::Value)>,
    ) -> Self {
        let kind = |value: &serde_json::Value| match value {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "boolean",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::Object(_) => "object",
        };
        self.diagnostic = Some(AdapterDiagnostic::Shape(ShapeDiagnostic {
            shape: kind(value),
            fields: value.as_object().map_or(0, |object| object.len()),
            rejected_field: rejected.map_or("none", |(field, _)| field),
            rejected_shape: rejected.map_or("none", |(_, value)| kind(value)),
        }));
        self
    }

    pub(crate) const fn with_diagnostics_from(mut self, cause: Self) -> Self {
        self.diagnostic = cause.diagnostic;
        self
    }

    pub(crate) const fn with_tool_identity_diagnostic(mut self, reason: &'static str) -> Self {
        self.diagnostic = Some(AdapterDiagnostic::ToolIdentity(reason));
        self
    }

    pub(crate) const fn with_schema_diagnostic(
        mut self,
        phase: &'static str,
        keyword: &'static str,
        schema_depth: usize,
        instance_depth: usize,
    ) -> Self {
        self.diagnostic = Some(AdapterDiagnostic::Schema(SchemaDiagnostic {
            phase,
            keyword,
            schema_depth,
            instance_depth,
        }));
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
        self.diagnostic = Some(AdapterDiagnostic::Json(JsonParseDiagnostic {
            reason,
            shape,
            line,
            column,
            bytes,
        }));
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
            diagnostic: None,
        }
    }

    pub(in crate::protocol::adapter) const fn compaction_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_COMPACTION_UNSUPPORTED,
            message: "Responses compaction history requires a native Responses route",
            parameter: Some("input"),
            diagnostic: None,
        }
    }

    pub(crate) const fn parameter_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_PARAMETER_UNSUPPORTED,
            message: "a request parameter has no lossless mapping on this route; use a compatible native endpoint",
            parameter: None,
            diagnostic: None,
        }
    }

    pub(crate) const fn continuation_missing() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISSING,
            message: "the adapter no longer has the prior response needed for this continuation",
            parameter: None,
            diagnostic: None,
        }
    }

    pub(crate) const fn agent_message_unavailable() -> Self {
        Self {
            message: "encrypted agent history could not be authenticated with this Relay profile and local key; use the original profile and key, or start a new chat if the old key was lost; rebuilding Relay cannot recover a lost key",
            parameter: Some("input.agent_message.encrypted_content"),
            ..Self::continuation_missing()
        }
    }

    pub(crate) const fn continuation_mismatch() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISMATCH,
            message: "the continuation belongs to a different model or source route",
            parameter: None,
            diagnostic: None,
        }
    }

    pub(crate) const fn unsupported_binding() -> Self {
        Self {
            code: error_codes::ADAPTER_BINDING_UNSUPPORTED,
            message: "the selected adapter cannot serve this client protocol",
            parameter: None,
            diagnostic: None,
        }
    }

    pub(in crate::protocol::adapter) const fn unsupported_tool() -> Self {
        Self {
            code: error_codes::ADAPTER_TOOL_UNSUPPORTED,
            message: "the selected source adapter cannot represent the requested tool type or format",
            parameter: None,
            diagnostic: None,
        }
    }

    pub(crate) const fn reasoning_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_REASONING_UNSUPPORTED,
            message: "the selected source adapter does not expose reasoning for this binding",
            parameter: None,
            diagnostic: None,
        }
    }

    pub(crate) const fn reasoning_metadata_conflict() -> Self {
        Self {
            code: error_codes::ADAPTER_METADATA_CONFLICT,
            message: "the source route and model reference declare incompatible reasoning effort levels",
            parameter: Some("reasoning.effort"),
            diagnostic: None,
        }
    }

    pub(crate) const fn output_limit_exceeded(parameter: &'static str) -> Self {
        Self {
            message: "the requested output token limit exceeds the selected route's declared maximum",
            parameter: Some(parameter),
            ..Self::invalid_request()
        }
    }

    pub(crate) const fn upstream_response_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "the upstream response cannot be represented by the selected client protocol",
            parameter: None,
            diagnostic: None,
        }
    }

    pub(crate) const fn upstream_tool_arguments_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "tool call arguments do not match the declared JSON Schema",
            parameter: Some("output.run_officejs.args.schema"),
            diagnostic: None,
        }
    }

    pub(crate) const fn tool_schema_invalid() -> Self {
        Self {
            message: "function tool parameters contain an invalid or unsupported JSON Schema",
            parameter: Some("tools.parameters"),
            ..Self::invalid_request()
        }
    }

    pub(crate) const fn upstream_tool_arguments_json_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "function tool arguments must be one complete JSON object; for script or command strings use the explicit relay-function-string-v1 frame instead of nested JSON escaping",
            parameter: None,
            diagnostic: None,
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
            diagnostic: None,
        }
    }

    pub(crate) const fn upstream_stream_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
            message: "the upstream stream is incomplete, invalid, or cannot be represented by the selected client protocol",
            parameter: None,
            diagnostic: None,
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)?;
        if let Some(parameter) = self
            .parameter
            .filter(|parameter| parameter.starts_with("output.run_officejs"))
        {
            let stage = if parameter.starts_with("output.run_officejs.args") {
                "tool_arguments"
            } else if parameter == "output.run_officejs.tool"
                || parameter == "output.run_officejs.call_id"
            {
                "tool_identity"
            } else {
                "tool_envelope"
            };
            write!(
                formatter,
                " (failure_stage={stage}, repair_skipped=executable_tool)"
            )?;
        }
        if let Some(AdapterDiagnostic::Json(diagnostic)) = self.diagnostic {
            write!(
                formatter,
                " (json_reason={}, payload_shape={}, parse_line={}, parse_column={}, payload_bytes={})",
                diagnostic.reason,
                diagnostic.shape,
                diagnostic.line,
                diagnostic.column,
                diagnostic.bytes
            )?;
        }
        if let Some(AdapterDiagnostic::Shape(diagnostic)) = self.diagnostic {
            write!(
                formatter,
                " (payload_shape={}, payload_fields={}, rejected_field={}, rejected_shape={})",
                diagnostic.shape,
                diagnostic.fields,
                diagnostic.rejected_field,
                diagnostic.rejected_shape
            )?;
        }
        if let Some(AdapterDiagnostic::Schema(diagnostic)) = self.diagnostic {
            write!(
                formatter,
                " (schema_phase={}, schema_keyword={}, schema_depth={}, instance_depth={})",
                diagnostic.phase,
                diagnostic.keyword,
                diagnostic.schema_depth,
                diagnostic.instance_depth
            )?;
        }
        if let Some(AdapterDiagnostic::ToolIdentity(reason)) = self.diagnostic {
            write!(formatter, " (tool_identity_reason={reason})")?;
        }
        Ok(())
    }
}

impl std::error::Error for AdapterError {}

pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

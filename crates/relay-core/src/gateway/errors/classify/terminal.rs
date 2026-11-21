use super::*;

/// Explicit request refusals share their meaning across HTTP and stream events.
/// Missing stream status is distinct from an explicit upstream server/auth error.
pub(in crate::gateway::errors) fn provider_request_terminal_category(
    status: Option<StatusCode>,
    payload: &Value,
) -> Option<&'static str> {
    let code = if let Some(code) = payload.get("code") {
        code.as_str()?
    } else {
        let error = ["/error", "/response/error", "/body/error", "/detail"]
            .into_iter()
            .find_map(|path| payload.pointer(path))?;
        error.get("code")?.as_str()?
    };
    match (code, status) {
        (
            "context_length_exceeded" | "context_window_exceeded" | "context_too_large",
            None | Some(StatusCode::BAD_REQUEST),
        ) => Some(error_codes::UPSTREAM_CONTEXT_TOO_LARGE),
        (
            "invalid_access_program" | "unsupported_access_program",
            None | Some(StatusCode::BAD_REQUEST),
        ) => Some(error_codes::UPSTREAM_ACCESS_PROGRAM_INVALID),
        ("access_program_not_enabled", None | Some(StatusCode::FORBIDDEN)) => {
            Some(error_codes::UPSTREAM_ACCESS_PROGRAM_NOT_ENABLED)
        }
        _ => None,
    }
}

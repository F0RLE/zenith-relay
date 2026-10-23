use crate::protocol::{AdapterError, AdapterResult};
use serde_json::Value;

pub(super) fn validate_unchanged(previous: &Value, terminal: &Value) -> AdapterResult<()> {
    let invalid = || AdapterError::upstream_stream_invalid();
    let (Some(previous), Some(terminal)) = (previous.as_object(), terminal.as_object()) else {
        return Err(invalid().with_parameter("response.output_item.changed"));
    };
    if previous == terminal {
        return Ok(());
    }
    let kind = previous.get("type").and_then(Value::as_str);
    for field in previous.keys().chain(terminal.keys()) {
        let before = previous.get(field);
        let after = terminal.get(field);
        // These optional fields are explicitly nullable in Responses. Omission
        // and null carry the same absent value, not a changed completed item.
        let nullable = matches!(
            (kind, field.as_str()),
            (Some("message"), "phase")
                | (Some("reasoning"), "encrypted_content")
                | (Some("function_call" | "custom_tool_call"), "caller")
        );
        if before == after
            || (nullable && before.is_none_or(Value::is_null) && after.is_none_or(Value::is_null))
        {
            continue;
        }
        // Keep diagnostics useful without echoing provider-controlled keys or values.
        let parameter = match field.as_str() {
            "content" => "response.output_item.content",
            "summary" => "response.output_item.summary",
            "encrypted_content" => "response.output_item.encrypted_content",
            "phase" => "response.output_item.phase",
            "status" => "response.output_item.status",
            "role" => "response.output_item.role",
            "arguments" => "response.output_item.arguments",
            "input" => "response.output_item.input",
            "caller" => "response.output_item.caller",
            "async" => "response.output_item.async",
            _ => "response.output_item.changed",
        };
        return Err(invalid().with_parameter(parameter));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

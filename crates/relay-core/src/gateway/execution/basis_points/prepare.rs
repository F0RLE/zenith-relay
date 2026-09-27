use super::catalog::{client_tools, requires_tool_call, selected_tools, ClientTool};
use super::codec::{
    as_input_items, basis_points_metadata, basis_points_reasoning_effort, sanitized_metadata,
    text_message,
};
use super::history::translate_input_items;
use super::response::has_encrypted_agent_message;
use super::{TRANSPORT_RETRY_HINT, TRANSPORT_TOOL};
use crate::protocol::AdapterError;
use serde_json::{Map, Value};

pub(super) fn tool_instructions(tools: &[ClientTool], request: &Value) -> String {
    if tools.is_empty() {
        return "This request is relayed by an external Responses API client, not by the live Excel workbook. The native run_officejs function is a transport endpoint owned by this proxy; the proxy intercepts it before execution, so it never runs Office code or changes the workbook. Do not call server-injected Excel, Office, connector, or workbook tools. Return the answer as assistant text.".to_string();
    }
    // Keep the injected catalog stable even when providers reorder the input
    // tool array. A deterministic prologue improves prompt-cache reuse and
    // prevents the model from seeing a different routing instruction for the
    // same set of client tools.
    let mut sorted_tools = tools.to_vec();
    sorted_tools.sort_by_key(ClientTool::key);
    let mut lines = Vec::with_capacity(sorted_tools.len());
    for tool in &sorted_tools {
        let description = tool
            .spec
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let schema = tool
            .spec
            .get("parameters")
            .or_else(|| tool.spec.get("input_schema"));
        let schema_text = schema.and_then(|value| serde_json::to_string(value).ok());
        let mut line = format!("- {} ({})", tool.key(), tool.kind);
        if !description.is_empty() {
            line.push_str(": ");
            line.push_str(description);
        }
        if tool.kind == "custom" {
            line.push_str(". It receives raw text in input; preserve every quote, backslash, newline, and space without another encoding layer");
            if let Some(format) = tool.spec.get("format") {
                if let Ok(format) = serde_json::to_string(format) {
                    line.push_str(". Input format: ");
                    line.push_str(&format);
                }
            }
        } else if let Some(schema_text) = schema_text {
            line.push_str(" JSON Schema: ");
            line.push_str(&schema_text);
        }
        lines.push(line);
    }
    let choice = request
        .get("tool_choice")
        .and_then(|value| serde_json::to_string(value).ok())
        .map(|value| format!(" Client tool_choice: {value}."))
        .unwrap_or_default();
    let parallel = request
        .get("parallel_tool_calls")
        .and_then(Value::as_bool)
        .filter(|parallel| !parallel)
        .map(|_| " Invoke at most one client tool in this response.")
        .unwrap_or_default();
    format!(
        "This request is relayed by an external Responses API client, not by the live Excel workbook. The native {TRANSPORT_TOOL} function is a transport endpoint owned by this proxy; the proxy intercepts it before execution, so it never runs Office code or changes the workbook. Other native server-injected Excel, Office, connector, workbook, list_skills, and web-search tools are unavailable. Never claim shell, filesystem, or workspace access is unavailable when the catalog contains a suitable client tool. Use exactly one outer native {TRANSPORT_TOOL} call for each client tool invocation. Set references to an array containing exactly one fully qualified client tool name from the catalog; references is the routing field, not a list of files or cells. Put only that tool payload in code. For function tools, code is one JSON object of arguments serialized once. For custom tools, code is the unchanged raw input text, without JSON encoding. Do not put a tool/args wrapper, JavaScript, Markdown fence, or another {TRANSPORT_TOOL} envelope in code. Historical calls may contain the old tool/args envelope; do not copy that format into new calls. The proxy converts this native call into the real client tool call, then replays the original {TRANSPORT_TOOL} identity with the client tool result on the next request. Interpret that result as the named client tool output. Never repeat a tool request whose output is already present. Example function envelope: {{\"summary\":\"Run client tool exec_command\",\"extended_summary\":\"Relay a shell command through the external client\",\"destructive\":false,\"references\":[\"exec_command\"],\"code\":\"{{\\\"cmd\\\":\\\"pwd\\\"}}\"}}. Example custom envelope: {{\"summary\":\"Run client tool apply_patch\",\"extended_summary\":\"Relay an unchanged patch through the external client\",\"destructive\":false,\"references\":[\"apply_patch\"],\"code\":\"*** Begin Patch\\n*** End Patch\"}}. The available client tools are authoritative:{choice}{parallel}\n{}",
        lines.join("\n")
    )
}

/// The upstream Basis Points adapter regenerates one malformed client-tool
/// relay before returning an error. Keep the same bounded recovery in Relay,
/// but only for errors that prove the model emitted the transport envelope;
/// malformed response bodies and missing terminal status are not retried.
pub(in crate::gateway::execution) fn should_retry_tool_relay(
    error: AdapterError,
    body: &[u8],
) -> bool {
    if error.code() != crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID {
        return false;
    }
    let Some(parameter) = error.parameter() else {
        return false;
    };
    if !(parameter.starts_with("output.run_officejs.") || parameter == "output.tool_call") {
        return false;
    }
    serde_json::from_slice::<Value>(body)
        .ok()
        .is_some_and(|response| response.get("status").and_then(Value::as_str) == Some("completed"))
}

/// Claim the one-shot malformed-tool regeneration without duplicating its
/// eligibility rules in the pooled and account-only execution loops.
pub(in crate::gateway::execution) fn take_tool_relay_retry(
    error: AdapterError,
    body: &[u8],
    attempted: &mut bool,
    parameter: &mut Option<&'static str>,
) -> bool {
    if *attempted || !should_retry_tool_relay(error, body) {
        return false;
    }
    *attempted = true;
    *parameter = error.parameter();
    true
}

/// Add the one-shot regeneration hint ahead of the conversation history. The
/// diagnostic contains only the adapter's safe parameter name; provider data
/// and tool arguments never enter the prompt or logs.
pub(in crate::gateway::execution) fn add_tool_relay_retry_hint(
    body: &mut Value,
    parameter: Option<&str>,
) -> bool {
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let diagnostic = parameter.unwrap_or("output.run_officejs");
    let hint = format!("{TRANSPORT_RETRY_HINT} Diagnostic: {diagnostic}");
    input.insert(0, text_message("developer", "input_text", hint));
    true
}

/// Prepare a client Responses request for the Basis Points executor.
pub(in crate::gateway::execution) fn prepare_request(
    request: &Value,
) -> Result<Value, AdapterError> {
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    // This transport does not implement server-side continuation. Dropping an
    // opaque predecessor would silently turn a continuation into a new chat.
    if object
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(AdapterError::parameter_unsupported_for(
            "previous_response_id",
        ));
    }
    if has_encrypted_agent_message(object.get("input")) {
        return Err(AdapterError::parameter_unsupported_for(
            "input.agent_message.encrypted_content",
        ));
    }
    let all_tools = client_tools(request);
    let callable = selected_tools(request, &all_tools);
    let tool_choice_requires_call = requires_tool_call(object.get("tool_choice"));
    if tool_choice_requires_call && callable.is_empty() {
        return Err(AdapterError::invalid_request().with_parameter("tool_choice"));
    }
    // Basis Points has a deliberately small Responses request contract. Do
    // not forward client-only fields such as `max_output_tokens`, sampling
    // controls, `parallel_tool_calls` or response formatting options: the
    // provider validates the body strictly and answers with a generic 422.
    let mut output = Map::new();
    if let Some(model) = object.get("model") {
        output.insert("model".to_string(), model.clone());
    }
    output.insert(
        "model_selection".to_string(),
        Value::String("explicit".to_string()),
    );
    output.insert("store".to_string(), Value::Bool(false));
    // Keep the upstream transport mode aligned with the client request. The
    // runtime still buffers a Basis Points response before translating it, but
    // the provider contract itself accepts the same stream flag as the
    // official adapter.
    output.insert(
        "stream".to_string(),
        Value::Bool(
            object
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    if object
        .get("context_management")
        .is_some_and(|value| match value {
            Value::Null => true,
            Value::Array(items) => items.is_empty(),
            Value::Object(fields) => fields.is_empty(),
            _ => false,
        })
    {
        // Empty context policies are rejected by the provider; omit them.
    } else if let Some(context_management) = object.get("context_management") {
        output.insert("context_management".to_string(), context_management.clone());
    }

    let mut input = translate_input_items(as_input_items(object.get("input")), &all_tools)?;
    let metadata = basis_points_metadata(object, &input);
    let mut prologue = Vec::new();
    if let Some(instructions) = object.get("instructions").and_then(Value::as_str) {
        if !instructions.trim().is_empty() {
            prologue.push(text_message(
                "developer",
                "input_text",
                instructions.to_string(),
            ));
        }
    }
    prologue.push(text_message(
        "developer",
        "input_text",
        tool_instructions(&callable, request),
    ));
    input.splice(0..0, prologue);
    output.insert("input".to_string(), Value::Array(input));

    if let Some(prompt_cache_key) = object
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.to_string()),
        );
    }
    let mut metadata_object = metadata.as_object().cloned().unwrap_or_default();
    if let Some(Value::Object(custom)) = sanitized_metadata(object.get("metadata")) {
        for (key, value) in custom {
            metadata_object.insert(key, value);
        }
    }
    output.insert("metadata".to_string(), Value::Object(metadata_object));

    // Basis Points does not accept a server-side `tools`/`tool_choice` body.
    // The official adapter supplies the client tool catalog in developer
    // instructions and lets the model emit the native run_officejs transport
    // call. Keeping these fields out of the strict upstream schema avoids a
    // generic 422 before generation.
    output.insert(
        "reasoning_effort".to_string(),
        Value::String(basis_points_reasoning_effort(object)),
    );
    Ok(Value::Object(output))
}

/// Prepare the upstream body and attach the one-shot relay hint when a retry is already claimed.
pub(in crate::gateway::execution) fn prepare_upstream(
    request: &Value,
    retry_parameter: Option<&str>,
) -> Result<Value, AdapterError> {
    let mut prepared = prepare_request(request)?;
    if retry_parameter.is_some() {
        add_tool_relay_retry_hint(&mut prepared, retry_parameter);
    }
    Ok(prepared)
}

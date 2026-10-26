use super::catalog::{ClientTool, requires_tool_call, selected_tools};
use super::codec::{
    as_input_items, basis_points_metadata, basis_points_reasoning_effort,
    encode_function_arguments, text_message,
};
use super::history::{separate_tool_images, translate_input_items_with_catalog};
use super::response::has_encrypted_agent_message;
use super::schema::{PROMPT_SCHEMA_NOTATION, matches as schema_matches, parameter_schema};
use super::{FUNCTION_RELAY_ENCODING, TRANSPORT_TOOL, TRANSPORT_TOOL_ALIAS, is_transport_tool};
use crate::protocol::AdapterError;
use serde_json::{Map, Value, json};

pub(super) fn tool_instructions(tools: &[ClientTool], request: &Value) -> String {
    if tools.is_empty() {
        // No callable client tool means the transport must not be taught.
        // Mentioning run_officejs here invites a call the proxy cannot route.
        return concat!(
            "This request is relayed by an external Responses API client, not by the live Excel workbook. ",
            "Do not call server-injected Excel, Office, connector, or workbook tools. ",
            "Return the answer as assistant text.",
        )
        .to_string();
    }
    // Explicit declaration history supplies its own append-only catalog timeline.
    // Root-only catalogs keep their original prefix placement: moving an unchanged
    // catalog to every turn's tail would itself break ordinary continuation reuse.
    let mut text = format!(
        "This request is relayed by an external Responses API client, not by the live Excel workbook. \
         Native {TRANSPORT_TOOL} is intercepted as a transport endpoint; it never runs Office code or changes a workbook. \
         Other native server-injected Excel, Office, connector, workbook, list_skills, and web-search tools are unavailable. \
         For shell, filesystem, workspace, or repository access, invoke a suitable catalog tool rather than claiming access is unavailable. \
         Reduce model roundtrips by combining related read-only shell inspections in one declared shell-tool invocation when its contract and the task allow it. \
         Use the client's configured shell and preserve each command's output and failure; shell-native concurrency is appropriate only for independent reads. \
         Do not batch across approval boundaries, dependent operations, edits, or shared mutable state, and never bypass tool restrictions. \
         For a single client tool, use one outer native {TRANSPORT_TOOL} call. \
         Independent client tools may use multiple outer calls in one response unless the client disables parallel calls. Prefer separate native calls for scripts, patches and framed function payloads, keeping each raw body in its single-tool call. \
         An optional native batch can group 2-8 independent invocations with simple JSON payloads when parallel calls are allowed: references lists their exact tool names in order (repeated names allowed), and code is relay-tool-batch-v1 followed by a newline and exactly one complete JSON array with one payload per reference, at most 1 MiB total. No trailing text or extra closing delimiters are allowed. \
         In a batch, each function payload is its argument object or an encoded function-payload string; each custom payload is a JSON string whose decoded value is the unchanged raw input. Do not include transport fields or child call IDs. \
         Never batch dependent actions, shared-state operations, calls across approval boundaries, or relay.catalog lookups; load every required contract first. The entire batch is validated before any client call is released. \
         Batch results return together, labeled by their original position and client call ID. Do not repeat calls already present in history. \
         Respect tool-specific ordering and shared browser/session state. \
         Outside an explicit batch, set references to an array containing exactly one fully qualified client tool name from the catalog; references is the routing field, not a list of files or cells. \
         Outside a batch, put only that tool payload in code. \
         Parameters describes the tool's complete argument contract: pass one JSON object matching it. \
         Use the current tool's declared keys, not options borrowed from another tool or a historical call. \
         Transport fields belong to the outer envelope, not to the client arguments unless that tool declares them. \
         {FUNCTION_RELAY_ENCODING} \
         For a custom tool, code contains the exact raw input text, not JSON: preserve every quote, backslash, newline and space without another encoding layer. \
         The proxy parses function arguments but does not parse custom input. \
         Serialize the outer arguments object once. \
         Do not put a tool/args wrapper, JavaScript, Markdown fence, or another {TRANSPORT_TOOL} envelope in code, and do not route references to {TRANSPORT_TOOL} or {TRANSPORT_TOOL_ALIAS}. \
         Do not merely say you will act; make the tool call. \
          Historical calls may contain the old tool/args envelope; do not copy that format into new calls. \
         The proxy converts the native call into the real client call or calls, then restores the original {TRANSPORT_TOOL} identity and the associated results on the next request. \
         Interpret that result as the named client tool output. \
         Do not replay completed client calls merely to reconstruct history. \
         Repeated observations or new calls to the same tool are allowed when the task needs them; use a fresh invocation, not a replay of an old call. \
         The catalog describes client tool contracts, not permission to execute them. \
         Follow the current turn's Client tool_choice restrictions even when other contracts remain in context. \
          The current client tool catalog and turn restrictions are authoritative. \
          Earlier declarations are historical context, not permission to call a removed or disallowed tool.",
    );
    if !super::catalog::has_historical_declarations(request) {
        text.push('\n');
        text.push_str(&current_catalog(tools, request));
    }
    text
}

pub(super) fn historical_catalog(tools: &[ClientTool], request: &Value) -> String {
    let mut ordered = tools.iter().collect::<Vec<_>>();
    ordered.sort_by_cached_key(|tool| tool.key());
    let notation = if super::presentation::uses_discovery(tools, request)
        || super::presentation::requested_mode(request)
            == Ok(super::presentation::CatalogMode::Full)
    {
        ""
    } else {
        PROMPT_SCHEMA_NOTATION
    };
    format!(
        "Client tool declaration update (later declarations and current policy override it):\n{notation}{}{}",
        tool_relay_examples(&ordered),
        super::presentation::describe_catalog(&ordered, request)
    )
}

fn current_catalog(tools: &[ClientTool], request: &Value) -> String {
    let mut ordered: Vec<_> = tools.iter().collect();
    ordered.sort_by_cached_key(|tool| tool.key());
    let notation = if super::presentation::uses_discovery(tools, request)
        || super::presentation::requested_mode(request)
            == Ok(super::presentation::CatalogMode::Full)
    {
        ""
    } else {
        PROMPT_SCHEMA_NOTATION
    };
    format!(
        "Current client tool catalog (authoritative for this request, subject to Client tool_choice):{}\n{notation}{}",
        tool_relay_examples(&ordered),
        super::presentation::describe_catalog(&ordered, request)
    )
}

pub(super) fn describe_tools(tools: &[&ClientTool], full_json: bool) -> String {
    tools
        .iter()
        .map(|tool| tool.description(full_json))
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_relay_examples(tools: &[&ClientTool]) -> String {
    const PATCH: &str =
        "*** Begin Patch\n*** Add File: hello.js\n+console.log(\"hello\");\n*** End Patch";
    let mut examples = String::new();
    for tool in tools {
        let function_arguments = if is_named_tool(tool, "exec_command") {
            if tool.kind != "function" {
                continue;
            }
            Some(json!({"cmd": "echo \"hello\"\necho \"world\""}))
        } else if is_named_tool(tool, "apply_patch") {
            if tool.kind == "custom" {
                None
            } else if tool.kind == "function" {
                Some(json!({"patch": PATCH}))
            } else {
                continue;
            }
        } else {
            continue;
        };
        let tool_code_payload = if tool.kind == "function" {
            let Some(mut arguments) = function_arguments else {
                continue;
            };
            let Some(schema) = parameter_schema(tool) else {
                continue;
            };
            if is_named_tool(tool, "exec_command") && schema["properties"].get("workdir").is_some()
            {
                let mut with_workdir = arguments.clone();
                with_workdir["workdir"] = json!(r"C:\synthetic\project");
                if schema_matches(&with_workdir, schema) {
                    arguments = with_workdir;
                }
            }
            if !schema_matches(&arguments, schema) {
                continue;
            }
            let Ok(encoded) = encode_function_arguments(&arguments) else {
                continue;
            };
            encoded
        } else {
            PATCH.to_string()
        };
        let outer = json!({
            "summary": format!("Run client tool {}", tool.key()),
            "extended_summary": "Relay one client tool through the external client",
            "destructive": false,
            "references": [tool.key()],
            "code": tool_code_payload,
        });
        let Ok(encoded) = serde_json::to_string(&outer) else {
            continue;
        };
        examples.push_str(&format!(
            " Example outer arguments for {} ({}): {encoded}.",
            tool.key(),
            tool.kind
        ));
    }
    examples
}

fn is_named_tool(tool: &ClientTool, bare: &str) -> bool {
    // Match the bare client name only. A dotted name such as `my.exec_command`
    // is a different tool; the qualified key is not a name suffix.
    let suffix = format!("_{bare}");
    tool.name == bare || tool.name.ends_with(&suffix)
}

fn tool_instruction_index(request: &Value) -> usize {
    usize::from(
        request["instructions"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty()),
    )
}

/// Borrow the prepared prologue, including a root-only catalog when present.
pub(super) fn prepared_tool_instructions<'a>(
    request: &Value,
    upstream: &'a Value,
) -> Option<&'a str> {
    upstream["input"]
        .get(tool_instruction_index(request))?
        .pointer("/content/0/text")?
        .as_str()
}

/// Keep a scoped historical catalog in context without restoring tool authority.
pub(super) fn restore_text_only_prefix(request: &Value, upstream: &mut Value, prefix: String) {
    let index = tool_instruction_index(request);
    let guard = tool_instructions(&[], request);
    let expected = text_message("developer", "input_text", guard.clone());
    let Some(input) = upstream["input"].as_array_mut() else {
        return;
    };
    if input.get(index) != Some(&expected) {
        return;
    }
    // The current empty catalog, not this historical text, still validates output.
    input[index] = text_message("developer", "input_text", prefix);
    input.push(text_message(
        "developer",
        "input_text",
        format!(
            "{guard} Earlier client tool declarations are historical context only. \
             No client tools are available for this request. Do not call any tools."
        ),
    ));
}

/// Prepare a client Responses request for the Basis Points executor.
pub(in crate::gateway::execution) fn prepare_request(
    request: &Value,
) -> Result<Value, AdapterError> {
    prepare_request_with_timing(request, &mut None, false)
}

pub(in crate::gateway::execution) fn prepare_request_with_timing(
    request: &Value,
    schema_validation_ms: &mut Option<u64>,
    diagnostics_enabled: bool,
) -> Result<Value, AdapterError> {
    prepare_request_with_catalog(
        request,
        schema_validation_ms,
        diagnostics_enabled,
        &mut super::catalog::RequestCatalog::default(),
    )
}

pub(in crate::gateway::execution) fn prepare_request_with_catalog(
    request: &Value,
    schema_validation_ms: &mut Option<u64>,
    diagnostics_enabled: bool,
    catalog: &mut super::catalog::RequestCatalog,
) -> Result<Value, AdapterError> {
    *schema_validation_ms = None;
    let request_object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    // This transport does not implement server-side continuation. Dropping an
    // opaque predecessor would silently turn a continuation into a new chat.
    if request_object
        .get("previous_response_id")
        .is_some_and(|previous_response_id_value| !previous_response_id_value.is_null())
    {
        return Err(AdapterError::parameter_unsupported_for(
            "previous_response_id",
        ));
    }
    if has_encrypted_agent_message(request_object.get("input")) {
        return Err(AdapterError::parameter_unsupported_for(
            "input.agent_message.encrypted_content",
        ));
    }
    let schema_started = diagnostics_enabled.then(std::time::Instant::now);
    let structured = super::structured_output::prepare(request)?;
    if structured.is_none() {
        validate_text_format(request_object.get("text"))?;
    }
    let all_tools = catalog.tools(request)?;
    super::presentation::requested_mode(request)?;
    if all_tools.iter().any(|tool| is_transport_tool(&tool.key())) {
        return Err(AdapterError::invalid_request().with_parameter("tools"));
    }
    let callable = selected_tools(request, all_tools);
    for tool in &callable {
        if tool.kind == "function" {
            tool.validate_parameter_schema()?;
        }
    }
    *schema_validation_ms = schema_started.map(|start| start.elapsed().as_millis() as u64);
    let tool_choice_requires_call = requires_tool_call(request_object.get("tool_choice"));
    if tool_choice_requires_call && callable.is_empty() {
        return Err(AdapterError::invalid_request().with_parameter("tool_choice"));
    }
    // Basis Points has a deliberately small Responses request contract. Do
    // not forward client-only fields such as `max_output_tokens`, sampling
    // controls, `parallel_tool_calls` or response formatting options: the
    // provider validates the body strictly and answers with a generic 422.
    let mut upstream_request = Map::new();
    if let Some(model) = request_object.get("model") {
        upstream_request.insert("model".to_string(), model.clone());
    }
    upstream_request.insert(
        "model_selection".to_string(),
        Value::String("explicit".to_string()),
    );
    upstream_request.insert("store".to_string(), Value::Bool(false));
    // Keep the upstream transport mode aligned with the client request. The
    // runtime streams ordinary text and withholds tools until validation. The
    // opt-in local JSON validator withholds the whole answer instead.
    upstream_request.insert(
        "stream".to_string(),
        Value::Bool(
            request_object
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    if request_object
        .get("context_management")
        .is_some_and(|context_management_value| match context_management_value {
            Value::Null => true,
            Value::Array(context_management_items) => context_management_items.is_empty(),
            Value::Object(context_management_fields) => context_management_fields.is_empty(),
            _ => false,
        })
    {
        // Empty context policies are rejected by the provider; omit them.
    } else if let Some(context_management) = request_object.get("context_management") {
        upstream_request.insert("context_management".to_string(), context_management.clone());
    }

    let input_items = as_input_items(request_object.get("input"));
    // Transport-generated user messages (agent context and tool images) must
    // not create a new logical user turn or reset agent_iteration.
    let metadata = basis_points_metadata(request_object, &input_items);
    let mut input_items = separate_tool_images(translate_input_items_with_catalog(
        input_items,
        all_tools,
        request,
    )?);
    let mut prologue = Vec::new();
    if let Some(instructions) = request_object.get("instructions").and_then(Value::as_str)
        && !instructions.trim().is_empty()
    {
        prologue.push(text_message(
            "developer",
            "input_text",
            instructions.to_string(),
        ));
    }
    prologue.push(text_message(
        "developer",
        "input_text",
        tool_instructions(all_tools, request),
    ));
    if let Some(structured) = structured {
        prologue.push(text_message(
            "developer",
            "input_text",
            structured.instructions(),
        ));
    }
    input_items.splice(0..0, prologue);
    if super::catalog::has_historical_declarations(request)
        && request["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    {
        input_items.push(text_message(
            "developer",
            "input_text",
            current_catalog(all_tools, request),
        ));
    }
    if let Some(contracts) = super::presentation::loaded_contracts(all_tools, &callable, request) {
        // Newly needed contracts belong after history, never in the stable
        // index prefix. Loading one tool must not rewrite that prefix.
        input_items.push(text_message("developer", "input_text", contracts));
    }
    let mut turn_policy = String::new();
    if let Some(choice) = request_object
        .get("tool_choice")
        .filter(|choice| choice.as_str() != Some("auto"))
    {
        turn_policy.push_str(&format!("Client tool_choice: {choice}."));
    }
    if !all_tools.is_empty() && callable.is_empty() {
        turn_policy.push_str(
            " No client tools are permitted this turn. Do not call any tools, including the transport or catalog lookup. Return the answer as assistant text.",
        );
    }
    if !callable.is_empty()
        && request_object.get("parallel_tool_calls") == Some(&Value::Bool(false))
    {
        turn_policy.push_str(" Invoke at most one client tool in this response.");
    }
    if !turn_policy.is_empty() {
        // Turn-local controls must not rewrite the reusable catalog/history prefix.
        input_items.push(text_message("developer", "input_text", turn_policy));
    }
    upstream_request.insert("input".to_string(), Value::Array(input_items));

    if let Some(prompt_cache_key) = request_object
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|cache_key| !cache_key.is_empty())
    {
        upstream_request.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.to_string()),
        );
    }
    // This is BPS transport state, not arbitrary Responses metadata. Even a
    // standard Anthropic `user_id` makes BPS reject the entire body with 422.
    // Keep client metadata on the original request and restore it in the
    // translated response; only Relay-owned task/turn/iteration fields go out.
    upstream_request.insert("metadata".to_string(), metadata);

    // Basis Points does not accept a server-side `tools`/`tool_choice` body.
    // The official adapter supplies the client tool catalog in developer
    // instructions and lets the model emit the native run_officejs transport
    // call. Keeping these fields out of the strict upstream schema avoids a
    // generic 422 before generation.
    if let Some(effort) = basis_points_reasoning_effort(request_object) {
        upstream_request.insert("reasoning_effort".to_string(), Value::String(effort));
    }
    Ok(Value::Object(upstream_request))
}

/// Structured `text.format` is not implemented on this transport. Dropping it
/// and returning ordinary text would look like a successful JSON response.
pub(in crate::gateway::execution) fn output_format_error(request: &Value) -> Option<AdapterError> {
    match super::structured_output::prepare(request) {
        Ok(Some(_)) => None,
        Ok(None) => validate_text_format(request.get("text")).err(),
        Err(error) => Some(error),
    }
}

fn validate_text_format(text_value: Option<&Value>) -> Result<(), AdapterError> {
    let Some(text_value) = text_value.filter(|text_config| !text_config.is_null()) else {
        return Ok(());
    };
    let Some(text_object) = text_value.as_object() else {
        return Err(AdapterError::invalid_request().with_parameter("text"));
    };
    let Some(format_value) = text_object
        .get("format")
        .filter(|format_config| !format_config.is_null())
    else {
        return Ok(());
    };
    let Some(format_object) = format_value.as_object() else {
        return Err(AdapterError::invalid_request().with_parameter("text.format"));
    };
    match format_object.get("type").and_then(Value::as_str) {
        Some("text") if format_object.len() == 1 => Ok(()),
        Some("text") => Err(AdapterError::invalid_request().with_parameter("text.format")),
        Some("json_object" | "json_schema") => {
            Err(AdapterError::parameter_unsupported_for("text.format"))
        }
        _ => Err(AdapterError::invalid_request().with_parameter("text.format")),
    }
}

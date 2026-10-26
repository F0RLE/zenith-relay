use super::*;
use serde_json::json;

pub(super) fn history_block(item: &Value) -> AdapterResult<Block> {
    checked(
        item,
        &[
            "type",
            "id",
            "execution",
            "call_id",
            "status",
            "arguments",
            "tools",
        ],
    )?;
    if item
        .get("status")
        .is_some_and(|status| !status.is_null() && status.as_str() != Some("completed"))
    {
        return Err(AdapterError::invalid_request().with_parameter("tool_search.status"));
    }
    let id = required_text(item, "call_id")?.into();
    if item["type"] == "tool_search_call" {
        checked(
            item,
            &["type", "id", "execution", "call_id", "status", "arguments"],
        )?;
        let arguments = item
            .get("arguments")
            .filter(|value| value.is_object())
            .ok_or_else(AdapterError::invalid_request)?;
        let target = ClientToolTarget::from_call(
            item.as_object().ok_or_else(AdapterError::invalid_request)?,
        )?;
        Ok(Block::ToolCall {
            id,
            name: target.upstream_name(),
            arguments: arguments.to_string(),
        })
    } else {
        checked(
            item,
            &["type", "id", "execution", "call_id", "status", "tools"],
        )?;
        let tools = item
            .get("tools")
            .filter(|value| value.is_array())
            .ok_or_else(AdapterError::invalid_request)?;
        Ok(Block::ToolResult {
            id,
            name: String::new(),
            content: vec![Block::Text(json!({"tools":tools}).to_string())],
            is_error: false,
            raw_text: false,
        })
    }
}

fn names(
    tools: &[Value],
    namespace: Option<&str>,
    deferred: bool,
    found: &mut BTreeSet<String>,
) -> AdapterResult<()> {
    for tool in tools {
        if tool["type"] == "namespace" {
            let name = required_text(tool, "name")?;
            let namespace =
                namespace.map_or_else(|| name.to_owned(), |parent| format!("{parent}.{name}"));
            names(
                tool.get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::unsupported_tool)?,
                Some(&namespace),
                deferred,
                found,
            )?;
        } else if tool["type"] != "tool_search"
            && (!deferred || optional_bool(tool, "defer_loading")? == Some(true))
        {
            let target = ClientToolTarget::from_definition(
                tool.as_object()
                    .ok_or_else(AdapterError::unsupported_tool)?,
                namespace,
            )?;
            found.insert(target.upstream_name());
        }
    }
    Ok(())
}

pub(super) fn filter_deferred(request: &mut Request, body: &Value) -> AdapterResult<()> {
    let input = body.get("input").and_then(Value::as_array);
    if !request
        .client_tools
        .values()
        .any(|target| target.kind == ResponsesToolKind::ToolSearch)
        && !input.is_some_and(|items| {
            items
                .iter()
                .any(|item| item["type"] == "tool_search_call" && item["execution"] == "client")
        })
    {
        return Ok(());
    }
    let catalog = super::super::contracts::request_tool_catalog(
        body.as_object().ok_or_else(AdapterError::invalid_request)?,
    )?
    .unwrap_or_default();
    let mut deferred = BTreeSet::new();
    names(&catalog, None, true, &mut deferred)?;
    let mut loaded = BTreeSet::new();
    for item in input.into_iter().flatten() {
        if matches!(
            item["type"].as_str(),
            Some("tool_search_output" | "additional_tools")
        ) {
            names(
                item.get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::invalid_request)?,
                None,
                false,
                &mut loaded,
            )?;
        }
    }
    if let Some(ToolChoice::Function(name)) = &request.tool_choice {
        loaded.insert(name.clone());
    }
    request
        .tools
        .retain(|tool| !deferred.contains(&tool.name) || loaded.contains(&tool.name));
    Ok(())
}

pub(super) fn lower_result(
    block: &mut Block,
    targets: &BTreeMap<String, ClientToolTarget>,
) -> AdapterResult<()> {
    let Block::ToolResult {
        name,
        content,
        raw_text,
        ..
    } = block
    else {
        return Ok(());
    };
    if !targets
        .get(name)
        .is_some_and(|target| target.kind == ResponsesToolKind::ToolSearch)
    {
        return Ok(());
    }
    let [Block::Text(text)] = content.as_slice() else {
        return Err(AdapterError::invalid_request());
    };
    let payload: Value = serde_json::from_str(text).map_err(|_| AdapterError::invalid_request())?;
    let mut loaded = BTreeSet::new();
    names(
        payload
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(AdapterError::invalid_request)?,
        None,
        false,
        &mut loaded,
    )?;
    // Exact definitions stay in the catalog and checkpoint, not twice in the model input.
    *content = vec![Block::Text(json!({"loaded_tools":loaded}).to_string())];
    *raw_text = false;
    Ok(())
}

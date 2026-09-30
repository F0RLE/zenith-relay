use super::*;

/// Classifies only explicit ChatGPT/Codex background operations. A model name,
/// reasoning effort, or ordinary client originator is deliberately insufficient
/// evidence because users can select those values manually.
pub(in crate::gateway) fn codex_background_request_kind(
    headers: &HeaderMap,
    request: &Value,
) -> Option<&'static str> {
    let metadata = background_metadata(headers, request);
    let has_codex_identity = codex_background_client(headers) || !metadata.is_empty();
    if !has_codex_identity {
        return None;
    }
    for document in &metadata {
        if let Some(kind) = metadata_kind_value(document) {
            return Some(kind);
        }
    }
    if let Some(kind) = headers
        .get("x-openai-subagent")
        .and_then(|value| value.to_str().ok())
        .and_then(metadata_kind_text)
    {
        return Some(kind);
    }
    // Prompt text is only a fallback when the client already sent turn metadata
    // or a subagent marker but omitted the structured operation. A normal
    // ChatGPT turn carries the same marker with request_kind=turn and must not
    // match from its originator or an ordinary prompt.
    if metadata.is_empty() && !headers.contains_key("x-openai-subagent") {
        return None;
    }
    let mut strings = Vec::new();
    collect_request_strings(request.get("input"), &mut strings);
    collect_request_strings(request.get("instructions"), &mut strings);
    strings.iter().find_map(|text| prompt_background_kind(text))
}

fn codex_background_client(headers: &HeaderMap) -> bool {
    let originator = headers
        .get("originator")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if originator.contains("codex")
        || matches!(originator.as_str(), "chatgpt desktop" | "chatgptdesktop")
    {
        return true;
    }
    headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| {
            value.starts_with("chatgptdesktop/")
                || value.starts_with("codex desktop/")
                || value.starts_with("codex-tui/")
                || value.starts_with("codex_cli_rs/")
        })
}

fn background_metadata(headers: &HeaderMap, request: &Value) -> Vec<Value> {
    let mut documents = Vec::new();
    if let Some(metadata) = headers
        .get("x-codex-turn-metadata")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_metadata_text)
    {
        documents.push(metadata);
    }
    if let Some(client_metadata) = request
        .get("client_metadata")
        .filter(|value| value.is_object())
    {
        if let Some(metadata) = client_metadata
            .get("x-codex-turn-metadata")
            .and_then(parse_metadata_value)
        {
            documents.push(metadata);
        }
        if client_metadata
            .as_object()
            .is_some_and(|object| object.keys().any(|key| metadata_operation_key(key)))
        {
            documents.push(client_metadata.clone());
        }
    }
    documents
}

fn metadata_operation_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "request_type"
            | "requesttype"
            | "task_type"
            | "tasktype"
            | "purpose"
            | "operation"
            | "kind"
            | "feature"
            | "thread_source"
            | "threadsource"
            | "turn_trigger"
            | "turntrigger"
    )
}

fn parse_metadata_text(value: &str) -> Option<Value> {
    serde_json::from_str::<Value>(value)
        .ok()
        .or_else(|| metadata_kind_text(value).map(|kind| Value::String(kind.to_string())))
}

fn parse_metadata_value(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => parse_metadata_text(text),
        Value::Object(_) | Value::Array(_) => Some(value.clone()),
        _ => None,
    }
}

fn metadata_kind_value(value: &Value) -> Option<&'static str> {
    match value {
        Value::String(_) => metadata_kind(value),
        Value::Object(object) => object.iter().find_map(|(key, value)| {
            let relevant = metadata_operation_key(key);
            if relevant {
                if let Some(kind) = metadata_kind(value) {
                    return Some(kind);
                }
            }
            if value.is_object() || value.is_array() {
                metadata_kind_value(value)
            } else {
                None
            }
        }),
        Value::Array(values) => values.iter().find_map(|value| {
            if value.is_object() || value.is_array() {
                metadata_kind_value(value)
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn metadata_kind(value: &Value) -> Option<&'static str> {
    let Value::String(text) = value else {
        return None;
    };
    metadata_kind_text(text)
}

fn metadata_kind_text(text: &str) -> Option<&'static str> {
    let normalized = text.trim().to_ascii_lowercase().replace('-', "_");
    let normalized = normalized.replace(' ', "_");
    match normalized.as_str() {
        "activity_summary" | "summarize_activity" | "thread_summary" | "thread_description" => {
            Some(CODEX_ACTIVITY_SUMMARY)
        }
        "task_title"
        | "generate_title"
        | "title_generation"
        | "thread_title"
        | "thread_title_reconsideration" => Some(CODEX_TASK_TITLE),
        _ => None,
    }
}

fn prompt_background_kind(text: &str) -> Option<&'static str> {
    let normalized = text.trim().to_ascii_lowercase();
    const ACTIVITY_PREFIXES: &[&str] = &[
        "summarize the activity",
        "summarise the activity",
        "you write the one-line activity update",
        "you are in a fork of an existing codex thread.",
    ];
    if ACTIVITY_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return Some(CODEX_ACTIVITY_SUMMARY);
    }
    const TITLE_PREFIXES: &[&str] = &[
        "generate a concise title for this task",
        "generate a title for this task",
        "generate a concise, single-line task title",
        "you are a helpful assistant. you will be presented with a user prompt, and your job is to provide a short title",
        "you are in a fork of a voice chat.",
    ];
    if TITLE_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return Some(CODEX_TASK_TITLE);
    }
    None
}

fn collect_request_strings(value: Option<&Value>, output: &mut Vec<String>) {
    match value {
        Some(Value::String(value)) => output.push(value.clone()),
        Some(Value::Array(values)) => values
            .iter()
            .for_each(|value| collect_request_strings(Some(value), output)),
        Some(Value::Object(object)) => object
            .values()
            .for_each(|value| collect_request_strings(Some(value), output)),
        _ => {}
    }
}

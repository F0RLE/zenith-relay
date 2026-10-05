use super::*;

mod legacy;
pub(in crate::gateway) use legacy::{
    remove_unpaired_responses_tool_call, repair_legacy_responses_call_ids,
};

pub(in crate::gateway) fn contains_tool_call_output(value: &Value) -> bool {
    let mut found = false;
    visit_response_items(value, &mut |object| {
        found |= object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "tool_search_output" || kind.ends_with("_call_output"));
    });
    found
}

/// Whether a Responses item contains a non-empty encrypted payload.
pub(in crate::gateway) fn responses_item_has_ciphertext(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(text)) => !text.trim().is_empty(),
        None | Some(Value::Null) => false,
        Some(_) => true,
    }
}

/// Returns the stable ids carried by Responses tool outputs. These ids are
/// stateful when the matching call is not included in the same request: the
/// provider that emitted the call is then the only safe owner.
/// IDs are length-bounded, but ownership checks must include the whole request.
/// Do not retain the tool payload itself.
pub(in crate::gateway) fn tool_call_output_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    visit_response_items(value, &mut |object| {
        let kind = object.get("type").and_then(Value::as_str);
        if kind.is_some_and(|kind| kind == "tool_search_output" || kind.ends_with("_call_output")) {
            if let Some(call_id) = bounded_tool_call_id(object.get("call_id")) {
                if seen.insert(call_id.clone()) {
                    ids.push(call_id);
                }
            }
        }
    });
    ids
}

/// Returns tool-call ids from a successful Responses response. Binding these
/// ids lets the next request remain on the candidate that created the call,
/// even when a client changes the selected model between turns.
pub(in crate::gateway) fn response_tool_call_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    visit_response_items(value, &mut |object| {
        let kind = object.get("type").and_then(Value::as_str);
        if kind.is_some_and(|kind| kind.ends_with("_call")) {
            for field in ["call_id", "id"] {
                if let Some(call_id) = bounded_tool_call_id(object.get(field)) {
                    if seen.insert(call_id.clone()) {
                        ids.push(call_id);
                    }
                }
            }
        }
    });
    ids
}

pub(in crate::gateway) fn unpaired_tool_output_ids(value: &Value) -> Vec<String> {
    let calls: HashSet<_> = response_tool_call_ids(value).into_iter().collect();
    tool_call_output_ids(value)
        .into_iter()
        .filter(|id| !calls.contains(id))
        .collect()
}

fn visit_response_items(value: &Value, inspect: &mut impl FnMut(&serde_json::Map<String, Value>)) {
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| visit_response_items(item, inspect)),
        Value::Object(object) => {
            let kind = object.get("type").and_then(Value::as_str);
            if !object.contains_key("role")
                && kind.is_none_or(|kind| kind == "response" || kind.starts_with("response."))
            {
                // Only protocol envelopes contain history. Tool schemas,
                // arguments and result content are data, even with call IDs.
                for field in ["input", "output", "response", "item"] {
                    if let Some(item) = object.get(field) {
                        visit_response_items(item, inspect);
                    }
                }
            } else {
                inspect(object);
            }
        }
        _ => {}
    }
}

fn bounded_tool_call_id(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && value.len() <= 256).then(|| value.to_string())
}

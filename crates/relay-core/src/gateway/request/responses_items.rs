use super::*;

mod legacy;
pub(in crate::gateway) use legacy::{
    remove_unpaired_responses_tool_call, repair_legacy_responses_call_ids,
};

pub(in crate::gateway) fn contains_tool_call_output(response_payload: &Value) -> bool {
    let mut found = false;
    visit_response_items(response_payload, &mut |response_object| {
        found |= response_object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "tool_search_output" || kind.ends_with("_call_output"));
    });
    found
}

/// Whether a Responses item contains a non-empty encrypted payload.
pub(in crate::gateway) fn responses_item_has_ciphertext(item_value: Option<&Value>) -> bool {
    match item_value {
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
pub(in crate::gateway) fn tool_call_output_ids(response_payload: &Value) -> Vec<String> {
    let mut tool_call_ids = Vec::new();
    let mut seen = HashSet::new();
    visit_response_items(response_payload, &mut |response_object| {
        let item_type = response_object.get("type").and_then(Value::as_str);
        if item_type.is_some_and(|item_type| {
            item_type == "tool_search_output" || item_type.ends_with("_call_output")
        }) {
            if let Some(call_id) = bounded_tool_call_id(response_object.get("call_id")) {
                if seen.insert(call_id.clone()) {
                    tool_call_ids.push(call_id);
                }
            }
        }
    });
    tool_call_ids
}

/// Returns tool-call ids from a successful Responses response. Binding these
/// ids lets the next request remain on the candidate that created the call,
/// even when a client changes the selected model between turns.
pub(in crate::gateway) fn response_tool_call_ids(response_payload: &Value) -> Vec<String> {
    let mut tool_call_ids = Vec::new();
    let mut seen = HashSet::new();
    visit_response_items(response_payload, &mut |response_object| {
        let item_type = response_object.get("type").and_then(Value::as_str);
        if item_type.is_some_and(|item_type| item_type.ends_with("_call")) {
            for field in ["call_id", "id"] {
                if let Some(call_id) = bounded_tool_call_id(response_object.get(field)) {
                    if seen.insert(call_id.clone()) {
                        tool_call_ids.push(call_id);
                    }
                }
            }
        }
    });
    tool_call_ids
}

pub(in crate::gateway) fn unpaired_tool_output_ids(response_payload: &Value) -> Vec<String> {
    let calls: HashSet<_> = response_tool_call_ids(response_payload)
        .into_iter()
        .collect();
    tool_call_output_ids(response_payload)
        .into_iter()
        .filter(|output_call_id| !calls.contains(output_call_id))
        .collect()
}

fn visit_response_items(
    response_value: &Value,
    inspect: &mut impl FnMut(&serde_json::Map<String, Value>),
) {
    match response_value {
        Value::Array(response_items) => response_items
            .iter()
            .for_each(|item_value| visit_response_items(item_value, inspect)),
        Value::Object(response_object) => {
            let item_type = response_object.get("type").and_then(Value::as_str);
            if !response_object.contains_key("role")
                && item_type.is_none_or(|item_type| {
                    item_type == "response" || item_type.starts_with("response.")
                })
            {
                // Only protocol envelopes contain history. Tool schemas,
                // arguments and result content are data, even with call IDs.
                for field in ["input", "output", "response", "item"] {
                    if let Some(nested_value) = response_object.get(field) {
                        visit_response_items(nested_value, inspect);
                    }
                }
            } else {
                inspect(response_object);
            }
        }
        _ => {}
    }
}

fn bounded_tool_call_id(candidate_value: Option<&Value>) -> Option<String> {
    let call_id_text = candidate_value?.as_str()?.trim();
    (!call_id_text.is_empty() && call_id_text.len() <= 256).then(|| call_id_text.to_string())
}

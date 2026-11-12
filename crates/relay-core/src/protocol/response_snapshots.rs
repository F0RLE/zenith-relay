//! Reconcile completed Responses items without interpreting opaque provider state.
use super::{AdapterError, AdapterResult};
use serde_json::Value;

/// Optional nullable Responses fields with equivalent omitted and null values.
pub(crate) fn is_nullable_item_field(kind: Option<&str>, field: &str) -> bool {
    matches!(
        (kind, field),
        (Some("message"), "phase")
            | (Some("reasoning"), "encrypted_content")
            | (Some("function_call" | "custom_tool_call"), "caller")
    )
}

pub(crate) fn validate_unchanged(previous: &Value, terminal: &Value) -> AdapterResult<()> {
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
        let nullable = is_nullable_item_field(kind, field);
        if before == after
            || (nullable && before.is_none_or(Value::is_null) && after.is_none_or(Value::is_null))
        {
            continue;
        }
        // Report fixed labels rather than provider-controlled keys or values.
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

/// Reconstruct sparse output only when completed snapshots prove its order.
pub(crate) fn ordered_completed_output(
    mut completed: Vec<(Option<u64>, Value)>,
) -> AdapterResult<Vec<Value>> {
    let invalid =
        || AdapterError::upstream_stream_invalid().with_parameter("response.output_item.changed");
    if completed.iter().any(|(_, item)| !item.is_object()) {
        return Err(invalid());
    }
    if completed.iter().all(|(index, _)| index.is_none()) {
        return Ok(completed.into_iter().map(|(_, item)| item).collect());
    }
    completed.sort_by_key(|(index, _)| *index);
    let mut output = Vec::with_capacity(completed.len());
    for (index, item) in completed {
        let index = usize::try_from(index.ok_or_else(invalid)?).map_err(|_| invalid())?;
        if let Some(previous) = output.get(index) {
            validate_unchanged(previous, &item)?;
        } else if index == output.len() {
            output.push(item);
        } else {
            return Err(invalid());
        }
    }
    Ok(output)
}

pub(crate) fn retain_completed_reasoning(
    previous: &Value,
    terminal: &mut Value,
) -> AdapterResult<()> {
    if previous.get("type").and_then(Value::as_str) != Some("reasoning")
        || terminal.get("type").and_then(Value::as_str) != Some("reasoning")
    {
        return Ok(());
    }
    let Some(completed @ Value::String(_)) = previous.get("encrypted_content") else {
        return Ok(());
    };
    if terminal
        .get("encrypted_content")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(AdapterError::upstream_stream_invalid()
            .with_parameter("response.output_item.encrypted_content"));
    }
    // Streaming clients must replay the ciphertext from output_item.done,
    // not an incomplete added item or the response's terminal snapshot.
    if terminal.get("encrypted_content") != Some(completed) {
        terminal["encrypted_content"] = completed.clone();
    }
    Ok(())
}

/// A terminal may fill missing snapshots, but cannot rewrite completed output.
pub(crate) fn reconcile_completed_output(
    completed: &[(Option<u64>, Value)],
    output: &mut [Value],
) -> AdapterResult<()> {
    let invalid =
        || AdapterError::upstream_stream_invalid().with_parameter("response.output_item.changed");
    let positional =
        completed.len() == output.len() && completed.iter().all(|(index, _)| index.is_none());
    let mut seen = std::collections::BTreeMap::new();
    for (position, (index, item)) in completed.iter().enumerate() {
        let index = match index {
            Some(index) => usize::try_from(*index).map_err(|_| invalid())?,
            None if positional && item.get("id").is_none() => position,
            None => {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(invalid)?;
                let mut matches = output.iter().enumerate().filter(|(_, candidate)| {
                    candidate.get("id").and_then(Value::as_str) == Some(id)
                });
                let (index, _) = matches.next().ok_or_else(invalid)?;
                if matches.next().is_some() {
                    return Err(invalid());
                }
                index
            }
        };
        if let Some(previous) = seen.insert(index, item) {
            validate_unchanged(previous, item)?;
        }
        let terminal = output.get_mut(index).ok_or_else(invalid)?;
        retain_completed_reasoning(item, terminal)?;
        // An otherwise unchanged reasoning item may first receive opaque state
        // in the terminal. Keep that state when no done ciphertext was supplied.
        if item["type"] == "reasoning"
            && item.get("encrypted_content").is_none_or(Value::is_null)
            && terminal["encrypted_content"].is_string()
        {
            let mut comparison = terminal.clone();
            match item.get("encrypted_content") {
                Some(value) => comparison["encrypted_content"] = value.clone(),
                None => {
                    comparison
                        .as_object_mut()
                        .ok_or_else(invalid)?
                        .remove("encrypted_content");
                }
            }
            validate_unchanged(item, &comparison)?;
        } else {
            validate_unchanged(item, terminal)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completed_reasoning_matches_index_or_unique_identity() {
        let item = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
            "encrypted_content":"synthetic-done-cipher"});
        for index in [None, Some(1)] {
            let mut output = vec![
                json!({"type":"message","id":"msg_synthetic"}),
                json!({"type":"reasoning","id":"rs_synthetic","summary":[],
                    "encrypted_content":"synthetic-terminal-cipher"}),
            ];
            reconcile_completed_output(&[(index, item.clone())], &mut output).unwrap();
            assert_eq!(output[1], item);
            assert_eq!(output[0], json!({"type":"message","id":"msg_synthetic"}));
        }
    }

    #[test]
    fn ambiguous_reasoning_identity_cannot_supply_replay_state() {
        let item = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
            "encrypted_content":"synthetic-private-cipher"});
        for (index, mut output) in [
            (Some(0), vec![json!({"type":"reasoning","id":"rs_other"})]),
            (Some(0), vec![json!({"type":"message","id":"rs_synthetic"})]),
            (Some(2), vec![item.clone()]),
            (Some(u64::MAX), vec![item.clone()]),
            (None, vec![]),
            (None, vec![item.clone(), item.clone()]),
        ] {
            let error =
                reconcile_completed_output(&[(index, item.clone())], &mut output).unwrap_err();
            assert_eq!(error.parameter(), Some("response.output_item.changed"));
            assert!(!error.to_string().contains("synthetic-private"));
        }
    }

    #[test]
    fn terminal_ciphertext_remains_when_no_completed_ciphertext_was_observed() {
        let terminal = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
            "encrypted_content":"synthetic-terminal-cipher"});
        let mut output = vec![terminal.clone()];
        let done = json!({"type":"reasoning","id":"rs_synthetic","summary":[]});
        reconcile_completed_output(&[(Some(0), done)], &mut output).unwrap();
        assert_eq!(output, vec![terminal]);
    }
}

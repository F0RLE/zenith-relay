//! Reconcile completed Responses items without interpreting opaque provider state.
use super::{AdapterError, AdapterResult};
use serde_json::Value;

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

pub(crate) fn retain_completed_reasoning_items(
    completed: &[(Option<u64>, Value)],
    output: &mut [Value],
) -> AdapterResult<()> {
    let invalid =
        || AdapterError::upstream_stream_invalid().with_parameter("response.output_item.changed");
    for (index, item) in completed {
        if item["type"] != "reasoning" || !item["encrypted_content"].is_string() {
            continue;
        }
        let index = match index {
            Some(index) => usize::try_from(*index).map_err(|_| invalid())?,
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
        let terminal = output.get_mut(index).ok_or_else(invalid)?;
        if terminal["type"] != "reasoning"
            || item
                .get("id")
                .is_some_and(|id| terminal.get("id") != Some(id))
        {
            return Err(invalid());
        }
        retain_completed_reasoning(item, terminal)?;
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
            retain_completed_reasoning_items(&[(index, item.clone())], &mut output).unwrap();
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
            let error = retain_completed_reasoning_items(&[(index, item.clone())], &mut output)
                .unwrap_err();
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
        retain_completed_reasoning_items(&[(Some(0), done)], &mut output).unwrap();
        assert_eq!(output, vec![terminal]);
    }
}

//! Reconcile completed Responses items without interpreting opaque provider state.
use super::{AdapterError, AdapterResult};
use serde_json::Value;

mod web_search;
pub(crate) use web_search::retain as retain_completed_web_search;

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
    let previous_item = previous;
    let invalid = |reason, before, after| {
        AdapterError::upstream_stream_invalid()
            .with_snapshot_diagnostic(previous, reason, before, after)
    };
    let (Some(previous), Some(terminal)) = (previous.as_object(), terminal.as_object()) else {
        return Err(invalid("item_shape", Some(previous), Some(terminal))
            .with_parameter("response.output_item.shape"));
    };
    if previous == terminal {
        return Ok(());
    }
    let kind = previous.get("type").and_then(Value::as_str);
    for field in previous.keys().chain(terminal.keys()) {
        let before = previous.get(field);
        let after = terminal.get(field);
        let nullable = is_nullable_item_field(kind, field);
        // Reasoning status is optional. An omitted status does not contradict
        // an explicitly completed snapshot; partial or invalid states still do.
        let omitted_completed_status = kind == Some("reasoning")
            && field == "status"
            && ((before.is_none() && after.is_some_and(|status| status == "completed"))
                || (after.is_none() && before.is_some_and(|status| status == "completed")));
        if before == after
            || (nullable && before.is_none_or(Value::is_null) && after.is_none_or(Value::is_null))
            || omitted_completed_status
        {
            continue;
        }
        if kind == Some("web_search_call")
            && field == "action"
            && let (Some(before), Some(after)) = (before, after)
        {
            web_search::validate(previous_item, before, after)?;
            continue;
        }
        // Report fixed labels rather than provider-controlled keys or values.
        let parameter = match field.as_str() {
            "id" => "response.output_item.id",
            "type" => "response.output_item.type",
            "name" => "response.output_item.name",
            "call_id" => "response.output_item.call_id",
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
            "namespace" => "response.output_item.namespace",
            "action" => "response.output_item.action",
            "results" => "response.output_item.results",
            "output" => "response.output_item.output",
            "error" => "response.output_item.error",
            "server_label" => "response.output_item.server_label",
            "tools" => "response.output_item.tools",
            "approval_request_id" => "response.output_item.approval_request_id",
            "container_id" => "response.output_item.container_id",
            "code" => "response.output_item.code",
            "result" => "response.output_item.result",
            _ => "response.output_item.changed",
        };
        let reason = match (before, after) {
            (None, _) => "field_added",
            (_, None) => "field_removed",
            _ => "field_changed",
        };
        return Err(invalid(reason, before, after).with_parameter(parameter));
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
        return Err(invalid().with_parameter("response.output_item.shape"));
    }
    if completed.iter().all(|(index, _)| index.is_none()) {
        return Ok(completed.into_iter().map(|(_, item)| item).collect());
    }
    completed.sort_by_key(|(index, _)| *index);
    let mut output = Vec::with_capacity(completed.len());
    for (index, mut item) in completed {
        let invalid_index = || invalid().with_parameter("response.output_item.index");
        let index =
            usize::try_from(index.ok_or_else(invalid_index)?).map_err(|_| invalid_index())?;
        if let Some(previous) = output.get(index) {
            retain_completed_web_search(previous, &mut item)?;
            validate_unchanged(previous, &item)?;
            output[index] = item;
        } else if index == output.len() {
            output.push(item);
        } else {
            return Err(invalid_index());
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
        let mismatch = |parameter, reason| {
            AdapterError::upstream_stream_invalid()
                .with_parameter(parameter)
                .with_snapshot_diagnostic(item, reason, Some(item), None)
        };
        let index = match index {
            Some(index) => usize::try_from(*index)
                .map_err(|_| mismatch("response.output_item.index", "index_out_of_bounds"))?,
            None if positional && item.get("id").is_none() => position,
            None => {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| mismatch("response.output_item.id", "missing_id"))?;
                let mut matches = output.iter().enumerate().filter(|(_, candidate)| {
                    candidate.get("id").and_then(Value::as_str) == Some(id)
                });
                let (index, _) = matches
                    .next()
                    .ok_or_else(|| mismatch("response.output_item.id", "unmatched_id"))?;
                if matches.next().is_some() {
                    return Err(mismatch("response.output_item.id", "ambiguous_id"));
                }
                index
            }
        };
        if let Some(previous) = seen.insert(index, item) {
            validate_unchanged(previous, item)?;
        }
        let terminal = output
            .get_mut(index)
            .ok_or_else(|| mismatch("response.output_item.index", "index_out_of_bounds"))?;
        retain_completed_reasoning(item, terminal)?;
        retain_completed_web_search(item, terminal)?;
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
    fn snapshot_mismatch_diagnostics_explain_changes_without_private_fields() {
        for (kind, field, parameter, expected_kind) in [
            (
                "web_search_call",
                "action",
                "response.output_item.action",
                "web_search_call",
            ),
            (
                "function_call",
                "namespace",
                "response.output_item.namespace",
                "function_call",
            ),
            (
                "synthetic-private-kind",
                "synthetic-private-key",
                "response.output_item.changed",
                "other",
            ),
        ] {
            let original = json!({"type":kind,"id":"synthetic-private-id"});
            let mut added = original.clone();
            added[field] = json!("synthetic-private-before");
            let mut changed = added.clone();
            changed[field] = json!("synthetic-private-after");
            for (before, after, reason, shapes) in [
                (
                    &original,
                    &added,
                    "field_added",
                    "before=missing, after=string",
                ),
                (
                    &added,
                    &original,
                    "field_removed",
                    "before=string, after=missing",
                ),
                (
                    &added,
                    &changed,
                    "field_changed",
                    "before=string, after=string",
                ),
            ] {
                let error = validate_unchanged(before, after).unwrap_err();
                assert_eq!(error.parameter(), Some(parameter));
                let diagnostic = error.to_string();
                assert!(
                    diagnostic.contains(&format!("item_kind={expected_kind}")),
                    "{diagnostic}"
                );
                assert!(
                    diagnostic.contains(&format!("snapshot_reason={reason}")),
                    "{diagnostic}"
                );
                assert!(diagnostic.contains(shapes), "{diagnostic}");
                assert!(!diagnostic.contains("synthetic-private"));
            }
        }
        let error = validate_unchanged(&json!({"type":"message"}), &Value::Null).unwrap_err();
        assert_eq!(error.parameter(), Some("response.output_item.shape"));
        assert!(error.to_string().contains("snapshot_reason=item_shape"));
        assert!(error.to_string().contains("before=object, after=null"));
    }

    #[test]
    fn snapshot_position_failures_have_distinct_safe_reasons() {
        let item = json!({"type":"reasoning","id":"synthetic-private-id","summary":[]});
        for (index, mut output, parameter, reason) in [
            (
                Some(2),
                vec![item.clone()],
                "response.output_item.index",
                "index_out_of_bounds",
            ),
            (None, vec![], "response.output_item.id", "unmatched_id"),
            (
                None,
                vec![item.clone(), item.clone()],
                "response.output_item.id",
                "ambiguous_id",
            ),
        ] {
            let error =
                reconcile_completed_output(&[(index, item.clone())], &mut output).unwrap_err();
            assert_eq!(error.parameter(), Some(parameter));
            let diagnostic = error.to_string();
            assert!(
                diagnostic.contains(&format!("snapshot_reason={reason}")),
                "{diagnostic}"
            );
            assert!(!diagnostic.contains("synthetic-private"));
        }
    }

    #[test]
    fn completed_reasoning_status_may_be_omitted_without_changing_state() {
        let omitted = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
            "encrypted_content":"synthetic-cipher"});
        let mut explicit = omitted.clone();
        explicit["status"] = json!("completed");
        for (item, terminal) in [
            (omitted.clone(), explicit.clone()),
            (explicit.clone(), omitted.clone()),
        ] {
            let mut output = vec![terminal.clone()];
            reconcile_completed_output(&[(Some(0), item)], &mut output).unwrap();
            assert_eq!(output, vec![terminal]);
        }
        for status in [json!("in_progress"), json!("incomplete"), Value::Null] {
            let mut invalid = omitted.clone();
            invalid["status"] = status;
            for (left, right) in [
                (&omitted, &invalid),
                (&invalid, &omitted),
                (&explicit, &invalid),
            ] {
                assert_eq!(
                    validate_unchanged(left, right).unwrap_err().parameter(),
                    Some("response.output_item.status")
                );
            }
        }
        for kind in ["message", "function_call", "custom_tool_call"] {
            let without = json!({"type":kind,"id":"synthetic"});
            let mut with = without.clone();
            with["status"] = json!("completed");
            assert!(validate_unchanged(&without, &with).is_err());
        }
    }

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
        for (index, mut output, parameter) in [
            (
                Some(0),
                vec![json!({"type":"reasoning","id":"rs_other"})],
                "response.output_item.id",
            ),
            (
                Some(0),
                vec![json!({"type":"message","id":"rs_synthetic"})],
                "response.output_item.type",
            ),
            (Some(2), vec![item.clone()], "response.output_item.index"),
            (
                Some(u64::MAX),
                vec![item.clone()],
                "response.output_item.index",
            ),
            (None, vec![], "response.output_item.id"),
            (
                None,
                vec![item.clone(), item.clone()],
                "response.output_item.id",
            ),
        ] {
            let error =
                reconcile_completed_output(&[(index, item.clone())], &mut output).unwrap_err();
            assert_eq!(error.parameter(), Some(parameter));
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

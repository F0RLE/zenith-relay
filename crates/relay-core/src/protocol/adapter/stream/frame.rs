use serde_json::{json, Value};

pub(crate) fn merge_usage(usage_snapshot: &mut Option<Value>, next_usage: &Value) {
    if !next_usage.is_object() {
        return;
    }
    let snapshot = usage_snapshot.get_or_insert_with(|| json!({}));
    merge_usage_fields(snapshot, next_usage);
}

fn merge_usage_fields(snapshot: &mut Value, next: &Value) {
    let Some(next_fields) = next.as_object() else {
        return;
    };
    if !snapshot.is_object() {
        *snapshot = json!({});
    }
    let previous_fields = snapshot
        .as_object_mut()
        .expect("usage snapshot is an object");
    let counts_changed = [
        "input_tokens",
        "prompt_tokens",
        "output_tokens",
        "completion_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "promptTokenCount",
        "candidatesTokenCount",
        "thoughtsTokenCount",
    ]
    .iter()
    .any(|field| {
        next_fields.get(*field).is_some_and(|value| {
            value.as_u64().is_some() && previous_fields.get(*field) != Some(value)
        })
    });
    if counts_changed {
        // A prior total belongs to the prior counters. Derive a fresh total
        // when a later cumulative update changes them without a new total.
        for field in ["total_tokens", "totalTokenCount"] {
            if next_fields.get(field).and_then(Value::as_u64).is_none() {
                previous_fields.remove(field);
            }
        }
    }
    for (field, value) in next_fields {
        // Providers use null for unavailable counters, including in terminal
        // snapshots. It must not erase consumption reported in earlier frames.
        if value.is_null() {
            continue;
        }
        if value.is_object() {
            merge_usage_fields(
                previous_fields
                    .entry(field.clone())
                    .or_insert_with(|| json!({})),
                value,
            );
        } else {
            previous_fields.insert(field.clone(), value.clone());
        }
    }
}

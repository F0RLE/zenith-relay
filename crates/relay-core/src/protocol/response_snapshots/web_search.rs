use super::*;

fn parameter(field: &str) -> &'static str {
    match field {
        "" => "response.output_item.action",
        "type" => "response.output_item.action.type",
        "query" => "response.output_item.action.query",
        "queries" => "response.output_item.action.queries",
        "sources" => "response.output_item.action.sources",
        "url" => "response.output_item.action.url",
        "pattern" => "response.output_item.action.pattern",
        _ => "response.output_item.action.changed",
    }
}

fn optional(kind: Option<&str>, field: &str) -> bool {
    matches!(
        (kind, field),
        (Some("search"), "query" | "queries" | "sources") | (Some("open_page"), "url")
    )
}

fn valid(field: &str, value: &Value) -> bool {
    match field {
        "query" | "url" => value.is_string(),
        "queries" => value
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string)),
        "sources" => value.as_array().is_some_and(|values| {
            values.iter().all(|value| {
                value.get("type").and_then(Value::as_str) == Some("url")
                    && value.get("url").is_some_and(Value::is_string)
            })
        }),
        _ => false,
    }
}

/// Hosted-search descriptions can receive optional metadata in the terminal.
/// Existing values, action kind and unknown extensions cannot be replaced.
pub(super) fn validate(item: &Value, before: &Value, after: &Value) -> AdapterResult<()> {
    let error = |field: &str, old: Option<&Value>, new: Option<&Value>| {
        let reason = match (old, new) {
            (None, _) => "field_added",
            (_, None) => "field_removed",
            _ => "field_changed",
        };
        AdapterError::upstream_stream_invalid()
            .with_parameter(parameter(field))
            .with_snapshot_diagnostic(item, reason, old, new)
    };
    let (Some(before), Some(after)) = (before.as_object(), after.as_object()) else {
        return Err(error("", Some(before), Some(after)));
    };
    let kind = before.get("type").and_then(Value::as_str);
    for field in before.keys().chain(after.keys()) {
        let old = before.get(field);
        let new = after.get(field);
        if old == new {
            continue;
        }
        if optional(kind, field) {
            let old_value = old.filter(|value| !value.is_null());
            let new_value = new.filter(|value| !value.is_null());
            match (old_value, new_value) {
                (None, None) => continue,
                (None, Some(value)) if valid(field, value) => continue,
                _ => {}
            }
            if field == "sources"
                && let (Some(old), Some(new)) = (old_value, new_value)
                && valid(field, old)
                && valid(field, new)
                && new.as_array().unwrap().starts_with(old.as_array().unwrap())
            {
                continue;
            }
        }
        return Err(error(field, old, new));
    }
    Ok(())
}

/// Preserve observed optional metadata if a later projection omits it.
pub(crate) fn retain(previous: &Value, terminal: &mut Value) -> AdapterResult<()> {
    if previous.get("type").and_then(Value::as_str) != Some("web_search_call")
        || terminal.get("type").and_then(Value::as_str) != Some("web_search_call")
    {
        return Ok(());
    }
    let (Some(before), Some(after)) = (previous.get("action"), terminal.get("action")) else {
        return Ok(());
    };
    if !before.is_object() || !after.is_object() {
        return validate(previous, before, after);
    }
    let kind = before.get("type").and_then(Value::as_str);
    let mut retained = after.clone();
    for (field, value) in before.as_object().unwrap() {
        if optional(kind, field)
            && !value.is_null()
            && retained.get(field).is_none_or(Value::is_null)
        {
            retained[field] = value.clone();
        }
    }
    validate(previous, before, &retained)?;
    terminal["action"] = retained;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn search(action: Value) -> Value {
        json!({"type":"web_search_call","id":"ws_synthetic","status":"completed","action":action})
    }

    #[test]
    fn optional_search_metadata_is_retained_and_can_be_enriched() {
        let original = search(json!({"type":"search","query":"synthetic query",
            "sources":[{"type":"url","url":"https://example.test/first"}]}));
        let mut projection = search(json!({"type":"search","query":null,"sources":null}));
        retain(&original, &mut projection).unwrap();
        assert_eq!(projection, original);
        let mut enriched = original.clone();
        enriched["action"]["sources"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"url","url":"https://example.test/second"}));
        super::super::validate_unchanged(&original, &enriched).unwrap();
        let ordered = super::super::ordered_completed_output(vec![
            (Some(0), original.clone()),
            (Some(0), enriched.clone()),
        ])
        .unwrap();
        assert_eq!(ordered, vec![enriched]);
        let open = search(json!({"type":"open_page","url":"https://example.test/first"}));
        let mut omitted = search(json!({"type":"open_page"}));
        retain(&open, &mut omitted).unwrap();
        assert_eq!(omitted, open);
    }

    #[test]
    fn conflicting_search_actions_keep_precise_private_safe_errors() {
        let original = search(json!({"type":"search","query":"synthetic-private-query",
            "queries":["synthetic-private-query"],
            "sources":[{"type":"url","url":"https://example.test/first"}]}));
        for (field, value, parameter) in [
            (
                "type",
                json!("open_page"),
                "response.output_item.action.type",
            ),
            (
                "query",
                json!("synthetic-private-other"),
                "response.output_item.action.query",
            ),
            (
                "queries",
                json!(["synthetic-private-other"]),
                "response.output_item.action.queries",
            ),
            ("sources", json!([]), "response.output_item.action.sources"),
            (
                "synthetic-private-field",
                json!(true),
                "response.output_item.action.changed",
            ),
        ] {
            let mut terminal = original.clone();
            terminal["action"][field] = value;
            let unchanged = terminal.clone();
            let error = retain(&original, &mut terminal).unwrap_err();
            assert_eq!(error.parameter(), Some(parameter));
            assert!(!error.to_string().contains("synthetic-private"));
            assert_eq!(terminal, unchanged);
        }
        for (field, invalid) in [
            ("query", json!(7)),
            ("queries", json!([7])),
            ("sources", json!([{"url":7}])),
        ] {
            let before = search(json!({"type":"search"}));
            let mut after = before.clone();
            after["action"][field] = invalid;
            assert!(retain(&before, &mut after).is_err());
        }
        let before = search(
            json!({"type":"find_in_page","url":"https://example.test/first","pattern":"needle"}),
        );
        let mut after = before.clone();
        after["action"]["pattern"] = json!("other");
        assert_eq!(
            retain(&before, &mut after).unwrap_err().parameter(),
            Some("response.output_item.action.pattern")
        );
    }
}

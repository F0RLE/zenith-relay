use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_ADVERTISED_CONTEXT_WINDOW: u64 = 16_000_000;

pub(crate) fn source_context_windows(
    manifest: &Value,
    configured_models: &BTreeSet<String>,
) -> BTreeMap<String, u64> {
    manifest
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("id")?.as_str()?.trim();
            if !configured_models
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(id))
            {
                return None;
            }
            let context_window = [
                model.get("context_length"),
                model.get("context_window"),
                model.get("max_context_tokens"),
                model.get("max_input_tokens"),
                model.pointer("/top_provider/context_length"),
                model.pointer("/metadata/context_length"),
            ]
            .into_iter()
            .flatten()
            .find_map(context_window)?;
            Some((id.to_ascii_lowercase(), context_window))
        })
        .collect()
}

pub(crate) fn context_window(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|window| (1..=MAX_ADVERTISED_CONTEXT_WINDOW).contains(window))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_common_context_fields_only_for_configured_models() {
        let configured = ["claude-fable-5", "grok-4.5", "gemini-3.5-flash"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let manifest = json!({
            "data": [
                {"id": "claude-fable-5", "context_length": 1_000_000},
                {"id": "grok-4.5", "context_length": 0, "context_window": "500000"},
                {"id": "gemini-3.5-flash", "top_provider": {"context_length": 1_048_576}},
                {"id": "not-configured", "context_length": 2_000_000},
                {"id": "invalid", "context_length": 0}
            ]
        });

        assert_eq!(
            source_context_windows(&manifest, &configured),
            BTreeMap::from([
                ("claude-fable-5".into(), 1_000_000),
                ("gemini-3.5-flash".into(), 1_048_576),
                ("grok-4.5".into(), 500_000),
            ])
        );
    }
}

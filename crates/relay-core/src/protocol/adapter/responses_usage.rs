//! Client-facing `usage` for a Responses client on a bridged route.
//!
//! A Responses client (Codex) rejects a `response.completed` whose `usage` lacks
//! `input_tokens`, `output_tokens`, or `total_tokens`. Messages never reports a
//! total, and Gemini and Chat Completions upstreams sometimes omit it. This only
//! shapes the body returned to the client; Relay's own usage records are read
//! from the upstream payload and are unaffected.

use serde_json::{Map, Value};

/// Completes a Responses usage object for the client.
///
/// When input and output are both known, a missing `total_tokens` is derived
/// (`derived_total`, otherwise input plus output) and an upstream-reported total
/// is kept. When either count is unknown the usage is `null`: an unknown count
/// is never invented as zero, and a partial object would fail the client parse.
pub(in crate::protocol::adapter) fn complete(
    mut usage: Map<String, Value>,
    derived_total: Option<u64>,
) -> Value {
    let count = |usage: &Map<String, Value>, field: &str| usage.get(field).and_then(Value::as_u64);
    let (Some(input), Some(output)) = (
        count(&usage, "input_tokens"),
        count(&usage, "output_tokens"),
    ) else {
        return Value::Null;
    };
    if count(&usage, "total_tokens").is_none() {
        let Some(total) = derived_total.or_else(|| input.checked_add(output)) else {
            return Value::Null;
        };
        usage.insert("total_tokens".into(), total.into());
    }
    Value::Object(usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn derives_a_missing_total() {
        let usage = complete(map(json!({"input_tokens": 3, "output_tokens": 2})), None);
        assert_eq!(usage["total_tokens"], 5);
    }

    #[test]
    fn keeps_a_reported_total() {
        let usage = complete(
            map(json!({"input_tokens": 3, "output_tokens": 2, "total_tokens": 9})),
            Some(100),
        );
        assert_eq!(usage["total_tokens"], 9);
    }

    #[test]
    fn prefers_the_protocol_total_over_input_plus_output() {
        let usage = complete(
            map(json!({"input_tokens": 3, "output_tokens": 2})),
            Some(11),
        );
        assert_eq!(usage["total_tokens"], 11);
    }

    #[test]
    fn unknown_counts_stay_unknown() {
        assert!(complete(Map::new(), None).is_null());
        assert!(complete(map(json!({"input_tokens": 3})), None).is_null());
        assert!(complete(map(json!({"output_tokens": 2})), Some(2)).is_null());
    }

    #[test]
    fn overflow_is_unknown_not_wrapped() {
        let usage = complete(
            map(json!({"input_tokens": u64::MAX, "output_tokens": 1})),
            None,
        );
        assert!(usage.is_null());
    }
}

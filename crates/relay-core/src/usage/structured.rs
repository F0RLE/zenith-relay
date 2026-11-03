//! Optional provider-reported counters, never inferred charges or extra tokens.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageDetails {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub server_tool_calls: BTreeMap<String, u64>,
    /// These are subsets of cache-write totals, not additional consumption.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cache_write_tokens_by_ttl: BTreeMap<String, u64>,
}

impl ProviderUsageDetails {
    pub(super) fn observe_payload(&mut self, payload: &Value) {
        for owner in [
            Some(payload),
            payload.get("message"),
            payload.get("response"),
            payload.pointer("/response/response"),
        ]
        .into_iter()
        .flatten()
        {
            let Some(usage) = owner.get("usage") else {
                continue;
            };
            if let Some(tools) = usage.get("server_tool_use") {
                for (field, kind) in [
                    ("web_search_requests", "web_search"),
                    ("web_fetch_requests", "web_fetch"),
                    ("code_execution_requests", "code_execution"),
                ] {
                    if let Some(count) = tools.get(field).and_then(Value::as_u64) {
                        observe(&mut self.server_tool_calls, kind, count);
                    }
                }
            }
            for (ttl, count) in reported_cache_write_buckets(usage) {
                if self.cache_write_tokens_by_ttl.contains_key(ttl)
                    || self.cache_write_tokens_by_ttl.len() < 24
                {
                    observe(&mut self.cache_write_tokens_by_ttl, ttl, count);
                }
            }
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.server_tool_calls.is_empty() && self.cache_write_tokens_by_ttl.is_empty()
    }
}

/// Provider-reported subsets only; no defaults, summation, or price inference.
pub(crate) fn reported_cache_write_buckets(usage: &Value) -> impl Iterator<Item = (&str, u64)> {
    [
        Some(usage),
        usage.get("input_tokens_details"),
        usage.get("prompt_tokens_details"),
        usage.get("inputTokensDetails"),
        usage.get("promptTokensDetails"),
    ]
    .into_iter()
    .flatten()
    .flat_map(|owner| {
        [
            (
                owner
                    .get("cache_creation")
                    .or_else(|| owner.get("cacheCreation")),
                "ephemeral_",
                "_input_tokens",
            ),
            (owner.get("cache_write_tokens_details"), "ttl_", "_tokens"),
        ]
        .into_iter()
        .filter_map(|(value, prefix, suffix)| {
            value?.as_object().map(|fields| (fields, prefix, suffix))
        })
    })
    .flat_map(|(fields, prefix, suffix)| {
        fields.iter().filter_map(move |(field, value)| {
            let ttl = field.strip_prefix(prefix)?.strip_suffix(suffix)?;
            let count = value.as_u64()?;
            valid_ttl(ttl).then_some((ttl, count))
        })
    })
}

fn observe(counters: &mut BTreeMap<String, u64>, key: &str, value: u64) {
    // Usage snapshots are cumulative within one attempt, not additive deltas.
    // Missing or provisional zero snapshots must not erase measured usage.
    counters
        .entry(key.to_owned())
        .and_modify(|count| *count = (*count).max(value))
        .or_insert(value);
}

fn valid_ttl(ttl: &str) -> bool {
    let Some(unit) = ttl.as_bytes().last() else {
        return false;
    };
    matches!(unit, b's' | b'm' | b'h')
        && ttl.len() <= 10
        && ttl[..ttl.len() - 1]
            .parse::<u32>()
            .is_ok_and(|value| value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn structured_counters_keep_zero_missing_and_cumulative_snapshots_distinct() {
        let mut tools = crate::ToolUseDiagnostics::default();
        tools.observe_stream_payload(&json!({"type":"message_start","message":{"usage":{
            "server_tool_use":{"web_search_requests":0},
            "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_15m_input_tokens":7}
        }}}));
        let observed = tools.provider_usage.as_ref().unwrap();
        assert_eq!(observed.server_tool_calls.get("web_search"), Some(&0));
        assert_eq!(observed.server_tool_calls.get("code_execution"), None);
        assert_eq!(observed.cache_write_tokens_by_ttl.get("15m"), Some(&7));
        assert!(tools.has_evidence());
        let persisted = serde_json::to_value(&tools).unwrap();
        let restored: crate::ToolUseDiagnostics = serde_json::from_value(persisted).unwrap();
        assert_eq!(restored.provider_usage, tools.provider_usage);
        tools.observe_stream_payload(&json!({"type":"message_delta","usage":{
            "server_tool_use":{"web_search_requests":2,"code_execution_requests":1}
        }}));
        tools.observe_stream_payload(&json!({"type":"message_delta","usage":{
            "server_tool_use":{"web_search_requests":0}
        }}));
        tools.set_terminal_response(&json!({"usage":{"input_tokens":4,"output_tokens":3}}));
        assert_eq!(
            tools
                .provider_usage
                .as_ref()
                .unwrap()
                .server_tool_calls
                .get("web_search"),
            Some(&2)
        );
        assert_eq!(
            tools
                .provider_usage
                .as_ref()
                .unwrap()
                .server_tool_calls
                .get("code_execution"),
            Some(&1)
        );
        let legacy: crate::ToolUseDiagnostics = serde_json::from_value(json!({
            "clientToolCount":0,"forwardedToolCount":0,"toolCallCount":0,"textOutput":false
        }))
        .unwrap();
        assert!(legacy.provider_usage.is_none());
    }

    #[test]
    fn responses_cache_buckets_preserve_zero_and_merge_without_adding_snapshots() {
        for field in ["input_tokens_details", "prompt_tokens_details"] {
            for envelope in [None, Some("response"), Some("message")] {
                let mut payload = json!({"usage":{}});
                payload["usage"][field] = json!({"cache_write_tokens_details":{
                    "ttl_30m_tokens":0,"ttl_12h_tokens":7
                }});
                if let Some(envelope) = envelope {
                    payload = json!({envelope:payload});
                }
                let mut tools = crate::ToolUseDiagnostics::default();
                tools.observe_provider_usage(&payload);
                tools.observe_provider_usage(&payload);
                tools.observe_provider_usage(&json!({"usage":{"input_tokens_details":{
                    "cache_write_tokens_details":{"ttl_12h_tokens":0}
                }}}));
                let observed = tools.provider_usage.as_ref().unwrap();
                assert_eq!(observed.cache_write_tokens_by_ttl.get("30m"), Some(&0));
                assert_eq!(observed.cache_write_tokens_by_ttl.get("12h"), Some(&7));
                assert_eq!(observed.cache_write_tokens_by_ttl.get("5m"), None);
                assert_eq!(observed.cache_write_tokens_by_ttl.len(), 2);
                assert!(tools.has_evidence());
                let stored = serde_json::to_value(&tools).unwrap();
                let restored: crate::ToolUseDiagnostics = serde_json::from_value(stored).unwrap();
                assert_eq!(restored.provider_usage, tools.provider_usage);
            }
        }
    }

    #[test]
    fn response_cache_buckets_ignore_invalid_fields_and_remain_bounded() {
        let mut details = ProviderUsageDetails::default();
        details.observe_payload(&json!({"usage":{"input_tokens_details":{
            "cache_write_tokens_details":{
                "ttl_30m_tokens":-1,"ttl_12h_tokens":"7","ttl_1h_tokens":null,
                "ttl_2h_tokens":18446744073709551616_u128,"ttl_0h_tokens":4,
                "ttl_private_tokens":9,"synthetic_private_field":9
            }
        }}}));
        assert!(details.is_empty());
        let buckets = (1..=32)
            .map(|minutes| (format!("ttl_{minutes}m_tokens"), json!(minutes)))
            .collect::<serde_json::Map<_, _>>();
        details.observe_payload(&json!({"usage":{"input_tokens_details":{
            "cache_write_tokens_details":buckets
        }}}));
        assert_eq!(details.cache_write_tokens_by_ttl.len(), 24);
        details.observe_payload(&json!({"usage":{"input_tokens_details":{
            "cache_write_tokens_details":{"ttl_1m_tokens":40}
        }}}));
        assert_eq!(details.cache_write_tokens_by_ttl.get("1m"), Some(&40));
        assert!(!serde_json::to_string(&details).unwrap().contains("private"));
    }

    #[test]
    fn structured_usage_never_captures_tool_names_arguments_or_arbitrary_fields() {
        let mut value = ProviderUsageDetails::default();
        value.observe_payload(&json!({"usage":{
            "server_tool_use":{"synthetic_private_tool":99,"web_search_requests":"7"},
            "cache_creation":{"ephemeral_secret_input_tokens":9,"ephemeral_0h_input_tokens":9}
        },"name":"synthetic-private","arguments":"synthetic-private"}));
        assert!(value.is_empty());
        assert!(!serde_json::to_string(&value).unwrap().contains("synthetic"));
    }
}

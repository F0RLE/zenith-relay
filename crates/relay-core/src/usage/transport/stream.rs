use serde::{Deserialize, Serialize};

/// Dispatch-relative stream observations, not additive CPU durations.
/// Only identity/client-decoded bodies are marked decoded; this is not a wire boundary.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamResponseDiagnostics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded_body_first_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sse_json_first_ms: Option<u64>,
    /// First typed reasoning activity; independent of generated text and TTFT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_reasoning_first_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_output_first_ms: Option<u64>,
    /// First nonempty upstream text delta, not reasoning or a terminal snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_text_first_ms: Option<u64>,
    /// First native function/custom tool item or nonempty argument delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_tool_first_ms: Option<u64>,
    /// First complete native tool item; not validated client-call readiness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_tool_first_complete_ms: Option<u64>,
}

impl StreamResponseDiagnostics {
    pub(crate) fn observe_output(&mut self, payload: &serde_json::Value, elapsed_ms: u64) {
        let kind = payload.get("type").and_then(serde_json::Value::as_str);
        let nonempty_delta = payload
            .get("delta")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|delta| !delta.is_empty());
        self.observe_delta(
            kind == Some("response.output_text.delta"),
            matches!(
                kind,
                Some(
                    "response.function_call_arguments.delta"
                        | "response.custom_tool_call_input.delta"
                )
            ),
            nonempty_delta,
            elapsed_ms,
        );
        if !matches!(
            kind,
            Some("response.output_item.added" | "response.output_item.done")
        ) {
            return;
        }
        let Some(item) = payload.get("item") else {
            return;
        };
        let field = match item.get("type").and_then(serde_json::Value::as_str) {
            Some("function_call") => "arguments",
            Some("custom_tool_call") => "input",
            _ => return,
        };
        self.upstream_tool_first_ms.get_or_insert(elapsed_ms);
        if kind == Some("response.output_item.done")
            && item
                .get("status")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|status| status == "completed")
            && ["id", "call_id"].iter().all(|key| {
                item.get(key)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| !id.is_empty())
            })
            && item.get(field).is_some_and(serde_json::Value::is_string)
        {
            self.upstream_tool_first_complete_ms
                .get_or_insert(elapsed_ms);
        }
    }

    pub(crate) fn observe_delta(
        &mut self,
        text: bool,
        tool: bool,
        nonempty: bool,
        elapsed_ms: u64,
    ) {
        if !nonempty {
            return;
        }
        if text {
            self.upstream_text_first_ms.get_or_insert(elapsed_ms);
        }
        if tool {
            self.upstream_tool_first_ms.get_or_insert(elapsed_ms);
        }
    }

    pub(crate) fn observe_reasoning(&mut self, payload: &serde_json::Value, elapsed_ms: u64) {
        if self.upstream_reasoning_first_ms.is_some() {
            return;
        }
        let reasoning = match payload.get("type").and_then(serde_json::Value::as_str) {
            Some("response.output_item.added" | "response.output_item.done") => {
                payload
                    .pointer("/item/type")
                    .and_then(serde_json::Value::as_str)
                    == Some("reasoning")
            }
            Some(
                "response.reasoning_summary_part.added"
                | "response.reasoning_summary_part.done"
                | "response.content_part.added"
                | "response.content_part.done",
            ) => matches!(
                payload
                    .pointer("/part/type")
                    .and_then(serde_json::Value::as_str),
                Some("summary_text" | "reasoning_text")
            ),
            Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
                payload
                    .get("delta")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|delta| !delta.is_empty())
            }
            _ => false,
        };
        if reasoning {
            self.upstream_reasoning_first_ms = Some(elapsed_ms);
        }
    }

    pub(crate) fn observe_body(&mut self, nonempty: bool, elapsed_ms: u64) {
        if nonempty {
            self.decoded_body_first_ms.get_or_insert(elapsed_ms);
        }
    }

    pub(crate) fn observe_event(
        &mut self,
        parsed_json: bool,
        meaningful_output: bool,
        elapsed_ms: u64,
    ) {
        if parsed_json {
            self.sse_json_first_ms.get_or_insert(elapsed_ms);
            if meaningful_output {
                self.upstream_output_first_ms.get_or_insert(elapsed_ms);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tool_completion_is_distinct_from_start_text_and_validation() {
        let mut timing = StreamResponseDiagnostics::default();
        timing.observe_output(&serde_json::json!({"type":"response.output_item.added","item":{"type":"function_call","id":"fc_test","call_id":"call_test","arguments":""}}), 0);
        timing.observe_output(
            &serde_json::json!({"type":"response.function_call_arguments.delta","delta":"{"}),
            20,
        );
        timing.observe_output(
            &serde_json::json!({"type":"response.function_call_arguments.done","arguments":"{}"}),
            30,
        );
        assert_eq!(timing.upstream_tool_first_ms, Some(0));
        assert_eq!(timing.upstream_tool_first_complete_ms, None);
        timing.observe_output(&serde_json::json!({"type":"response.output_item.done","item":{"type":"function_call","id":"fc_test","call_id":"call_test","arguments":"{}","status":"completed"}}), 40);
        timing.observe_output(
            &serde_json::json!({"type":"response.output_text.delta","delta":"text"}),
            50,
        );
        timing.observe_output(
            &serde_json::json!({"type":"response.output_text.delta","delta":"later"}),
            80,
        );
        assert_eq!(timing.upstream_tool_first_complete_ms, Some(40));
        assert_eq!(timing.upstream_text_first_ms, Some(50));
        let json = serde_json::to_value(timing).unwrap();
        assert_eq!(json["upstreamToolFirstMs"], 0);
        assert!(json.get("arguments").is_none());
    }

    #[test]
    fn reasoning_snapshots_and_partial_tools_do_not_invent_complete_items() {
        let mut timing = StreamResponseDiagnostics::default();
        for event in [
            serde_json::json!({"type":"response.reasoning_text.delta","delta":"reasoning"}),
            serde_json::json!({"type":"response.output_text.delta","delta":""}),
            serde_json::json!({"type":"response.completed","response":{"output":[{"type":"function_call","id":"fc","call_id":"call","arguments":"{}"}]}}),
            serde_json::json!({"type":"response.output_item.done","item":{"type":"function_call","id":"fc","call_id":"call","arguments":"{}","status":"incomplete"}}),
            serde_json::json!({"type":"response.output_item.done","item":{"type":"function_call","arguments":"{}"}}),
        ] {
            timing.observe_output(&event, 10);
        }
        assert_eq!(timing.upstream_tool_first_complete_ms, None);
        assert_eq!(timing.upstream_text_first_ms, None);
        let old: StreamResponseDiagnostics = serde_json::from_str("{}").unwrap();
        assert_eq!(old.upstream_tool_first_ms, None);
        assert_eq!(old.upstream_tool_first_complete_ms, None);
    }

    #[test]
    fn custom_tool_completion_preserves_empty_opaque_input_and_first_observation() {
        let mut timing = StreamResponseDiagnostics::default();
        let complete = serde_json::json!({"type":"response.output_item.done","item":{"type":"custom_tool_call","id":"ct_test","call_id":"call_test","input":""}});
        timing.observe_output(&complete, 0);
        timing.observe_output(&complete, 50);
        assert_eq!(timing.upstream_tool_first_ms, Some(0));
        assert_eq!(timing.upstream_tool_first_complete_ms, Some(0));
    }

    #[test]
    fn stream_stages_preserve_first_observation_and_measured_zero() {
        let mut timing = StreamResponseDiagnostics::default();
        timing.observe_body(false, 0);
        timing.observe_event(false, false, 1);
        assert_eq!(timing, StreamResponseDiagnostics::default());
        timing.observe_body(true, 0);
        timing.observe_body(true, 10);
        timing.observe_event(true, false, 12);
        assert_eq!(timing.upstream_output_first_ms, None);
        timing.observe_event(true, true, 35);
        timing.observe_event(true, true, 99);
        assert_eq!(timing.decoded_body_first_ms, Some(0));
        assert_eq!(timing.sse_json_first_ms, Some(12));
        assert_eq!(timing.upstream_output_first_ms, Some(35));
        let json = serde_json::to_value(&timing).unwrap();
        assert_eq!(json["decodedBodyFirstMs"], 0);
        assert!(json.get("rawBodyFirstMs").is_none());
    }

    #[test]
    fn old_or_unobserved_stages_remain_unknown() {
        let timing: StreamResponseDiagnostics = serde_json::from_str("{}").unwrap();
        assert_eq!(timing, StreamResponseDiagnostics::default());
        assert_eq!(serde_json::to_value(timing).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn reasoning_activity_preserves_its_own_first_observation() {
        let events = [
            serde_json::json!({"type":"response.output_item.added","item":{"type":"reasoning","summary":[]}}),
            serde_json::json!({"type":"response.output_item.done","item":{"type":"reasoning"}}),
            serde_json::json!({"type":"response.reasoning_summary_part.added","part":{"type":"summary_text","text":""}}),
            serde_json::json!({"type":"response.content_part.added","part":{"type":"reasoning_text","text":""}}),
            serde_json::json!({"type":"response.reasoning_text.delta","delta":"Thinking"}),
            serde_json::json!({"type":"response.reasoning_summary_text.delta","delta":" "}),
        ];
        for event in events {
            let mut timing = StreamResponseDiagnostics::default();
            timing.observe_reasoning(&event, 0);
            timing.observe_reasoning(&event, 99);
            assert_eq!(timing.upstream_reasoning_first_ms, Some(0));
            assert_eq!(timing.upstream_output_first_ms, None);
            assert_eq!(timing.sse_json_first_ms, None);
            assert_eq!(
                serde_json::to_value(timing).unwrap()["upstreamReasoningFirstMs"],
                0
            );
        }
    }

    #[test]
    fn unrelated_or_terminal_events_do_not_invent_reasoning() {
        for event in [
            serde_json::json!({"type":"response.created"}),
            serde_json::json!({"type":"response.output_item.added","item":{"type":"message"}}),
            serde_json::json!({"type":"response.reasoning_summary_part.added","part":{}}),
            serde_json::json!({"type":"response.content_part.added","part":{"type":"output_text","text":"Hello"}}),
            serde_json::json!({"type":"response.reasoning_text.delta","delta":""}),
            serde_json::json!({"type":"response.reasoning_text.delta","delta":{"text":"not a delta"}}),
            serde_json::json!({"type":"response.output_text.delta","delta":"Hello"}),
            serde_json::json!({"type":"response.completed","response":{"output":[{"type":"reasoning"}],"usage":{"output_tokens":5}}}),
            serde_json::json!({"type":"response.failed"}),
        ] {
            let mut timing = StreamResponseDiagnostics::default();
            timing.observe_reasoning(&event, 10);
            assert_eq!(timing, StreamResponseDiagnostics::default(), "{event}");
        }
    }
}

use serde::{Deserialize, Serialize};

/// Dispatch-relative HTTP body/parser observations, not additive CPU durations.
/// Only identity/client-decoded bodies are marked decoded; this is not a wire boundary.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamResponseDiagnostics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded_body_first_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sse_json_first_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_output_first_ms: Option<u64>,
}

impl StreamResponseDiagnostics {
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
}

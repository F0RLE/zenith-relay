//! Optional native Codex backend measurements, never inferred from Relay TTFT.
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexServerTiming {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overhead_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_iapi_ttft_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_service_ttft_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_iapi_tbt_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_service_tbt_ms: Option<Number>,
}

impl CodexServerTiming {
    pub(crate) fn from_event(
        value: &Value,
        response_id: Option<&str>,
        first_response_on_connection: bool,
    ) -> Option<Self> {
        if value.get("type").and_then(Value::as_str) != Some("responsesapi.websocket_timing") {
            return None;
        }
        let event_id = value
            .get("response_id")
            .or_else(|| value.pointer("/response/id"));
        match event_id {
            Some(id)
                if id.as_str().filter(|id| !id.is_empty()) != response_id
                    || response_id.is_none() =>
            {
                return None;
            }
            None if !first_response_on_connection => return None,
            _ => {}
        }
        let metrics = value.get("timing_metrics")?.as_object()?;
        let read = |name| {
            let number = metrics.get(name)?.as_number()?;
            let milliseconds = number.as_f64()?;
            (milliseconds.is_finite() && (0.0..=86_400_000.0).contains(&milliseconds))
                .then(|| number.clone())
        };
        let timing = Self {
            overhead_ms: read("responses_duration_excl_engine_and_client_tool_time_ms"),
            inference_ms: read("engine_service_total_ms"),
            engine_iapi_ttft_ms: read("engine_iapi_ttft_total_ms"),
            engine_service_ttft_ms: read("engine_service_ttft_total_ms"),
            engine_iapi_tbt_ms: read("engine_iapi_tbt_across_engine_calls_ms"),
            engine_service_tbt_ms: read("engine_service_tbt_across_engine_calls_ms"),
        };
        (timing != Self::default()).then_some(timing)
    }
}

impl super::TransportDiagnostics {
    pub(crate) fn observe_codex_server_timing(&mut self, event: &Value, response_id: Option<&str>) {
        if let Some(timing) =
            CodexServerTiming::from_event(event, response_id, self.connection_reused == Some(false))
        {
            self.codex_server_timing.get_or_insert(timing);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_only_measured_numbers_and_preserves_fractional_milliseconds() {
        let timing = CodexServerTiming::from_event(
            &json!({
                "type":"responsesapi.websocket_timing", "timing_metrics": {
                    "responses_duration_excl_engine_and_client_tool_time_ms": 0,
                    "engine_service_total_ms": 1200,
                    "engine_iapi_ttft_total_ms": -1,
                    "engine_service_ttft_total_ms": "200",
                    "engine_iapi_tbt_across_engine_calls_ms": 2.75,
                    "engine_service_tbt_across_engine_calls_ms": 86400001,
                    "private_extension": "must not survive"
                }
            }),
            None,
            true,
        )
        .unwrap();
        assert_eq!(timing.overhead_ms, Some(0.into()));
        assert_eq!(timing.inference_ms, Some(1200.into()));
        assert_eq!(timing.engine_iapi_tbt_ms.unwrap().as_f64(), Some(2.75));
        assert!(timing.engine_iapi_ttft_ms.is_none());
        assert!(timing.engine_service_ttft_ms.is_none());
        assert!(timing.engine_service_tbt_ms.is_none());
    }

    #[test]
    fn reused_connections_require_an_exact_response_identity() {
        let mut value = json!({"type":"responsesapi.websocket_timing",
            "timing_metrics":{"engine_service_total_ms": 42}});
        assert!(CodexServerTiming::from_event(&value, Some("current"), false).is_none());
        value["response_id"] = json!("previous");
        assert!(CodexServerTiming::from_event(&value, Some("current"), false).is_none());
        value["response_id"] = json!("current");
        assert!(CodexServerTiming::from_event(&value, Some("current"), false).is_some());
        assert!(CodexServerTiming::from_event(&value, None, true).is_none());
        value["response_id"] = Value::Null;
        assert!(CodexServerTiming::from_event(&value, None, true).is_none());
    }

    #[test]
    fn absence_and_unrelated_events_do_not_create_zero_measurements() {
        for value in [
            json!({}),
            json!({"type":"response.completed"}),
            json!({"type":"responsesapi.websocket_timing","timing_metrics":{}}),
        ] {
            assert!(CodexServerTiming::from_event(&value, None, true).is_none());
        }
    }
}

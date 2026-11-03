use super::contracts::TranslatedResponse;
use serde_json::Value;

mod frame;

#[cfg(test)]
use super::translation::TranslationStream;
pub(crate) use frame::merge_usage;

pub enum AdapterStreamBridge {
    Translated(Box<super::translation::TranslationStream>),
}

impl AdapterStreamBridge {
    pub fn push(&mut self, bytes: &[u8]) {
        match self {
            Self::Translated(bridge) => bridge.push(bytes),
        }
    }
    pub fn finish(&mut self) {
        match self {
            Self::Translated(bridge) => bridge.finish(),
        }
    }
    pub(crate) fn awaiting_upstream_end(&self) -> bool {
        match self {
            Self::Translated(bridge) => bridge.awaiting_upstream_end(),
        }
    }
    pub(crate) fn abort(&mut self) {
        match self {
            Self::Translated(bridge) => bridge.abort(),
        }
    }
    pub fn pop_output(&mut self) -> Option<Vec<u8>> {
        match self {
            Self::Translated(bridge) => bridge.pop_output(),
        }
    }
    pub fn completed(&self) -> Option<&TranslatedResponse> {
        match self {
            Self::Translated(bridge) => bridge.completed(),
        }
    }
    pub(crate) fn take_completed(&mut self) -> Option<TranslatedResponse> {
        match self {
            Self::Translated(bridge) => bridge.take_completed(),
        }
    }
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Translated(bridge) => bridge.is_terminal(),
        }
    }
    pub fn take_upstream_error(&mut self) -> Option<Value> {
        match self {
            Self::Translated(bridge) => bridge.take_upstream_error(),
        }
    }
}

#[cfg(test)]
mod gemini_stream_tests {
    use super::*;
    use crate::protocol::adapter::gemini::prepare_responses_to_gemini;
    use serde_json::json;

    #[test]
    fn gemini_stream_emits_responses_events_and_usage() {
        let request = prepare_responses_to_gemini(
            &json!({"input": "Hello"}),
            "gemini-test",
            true,
            "route",
            "request-42",
        )
        .unwrap();
        let mut bridge = TranslationStream::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}

data: {"candidates":[{"content":{"parts":[{"text":"Hello world"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5}}

"#,
        );
        assert!(bridge.completed().is_none());
        bridge.finish();
        let output = std::iter::from_fn(|| bridge.pop_output())
            .flat_map(|frame| {
                String::from_utf8(frame)
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(bridge.is_terminal());
        assert!(output.contains("response.output_text.delta"));
        assert!(output.contains("Hello world"));
        assert!(output.contains("\"total_tokens\":5"));
    }

    #[test]
    fn gemini_stream_emits_tool_events_and_captures_continuation() {
        let request = prepare_responses_to_gemini(
            &json!({
                "input": "inspect",
                "tools": [{"type":"function","name":"run","parameters":{"type":"object"}}]
            }),
            "gemini-test",
            true,
            "route",
            "request-tool",
        )
        .unwrap();
        let mut bridge = TranslationStream::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"run","args":{"command":"pwd"},"id":"call-1"}}]}}]}

data: {"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":2}}

"#,
        );
        bridge.finish();
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");
        assert!(bridge.completed().is_some());
        assert!(output.contains("response.function_call_arguments.delta"));
        assert!(output.contains("response.function_call_arguments.done"));
        assert_eq!(
            bridge.completed().unwrap().response_body["output"][0]["type"],
            "function_call"
        );
        assert_eq!(
            bridge
                .completed()
                .unwrap()
                .continuation
                .portable_history
                .as_ref()
                .unwrap()
                .len(),
            2
        );
        let native_pointer = bridge.completed().unwrap().continuation.native_turns[&1].as_ptr();
        let history_pointer = bridge
            .completed()
            .unwrap()
            .continuation
            .portable_history
            .as_ref()
            .unwrap()
            .as_ptr();
        let completed = bridge.take_completed().unwrap();
        assert_eq!(
            completed.continuation.native_turns[&1].as_ptr(),
            native_pointer
        );
        assert_eq!(
            completed
                .continuation
                .portable_history
                .as_ref()
                .unwrap()
                .as_ptr(),
            history_pointer
        );
        assert!(bridge.completed().is_none());
        assert!(bridge.take_completed().is_none());
        assert!(bridge.is_terminal());
        bridge.push(b"data: {}\n\n");
        bridge.finish();
        assert!(bridge.pop_output().is_none());
        assert!(bridge.take_completed().is_none());
    }

    #[test]
    fn gemini_stream_reassembles_vertex_partial_function_arguments() {
        let request = prepare_responses_to_gemini(
            &json!({
                "input": "weather",
                "tools": [{
                    "type": "function",
                    "name": "get_weather",
                    "parameters": {"type":"object"}
                }]
            }),
            "gemini-test",
            true,
            "route",
            "partial-tool",
        )
        .unwrap();
        let mut bridge = TranslationStream::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","willContinue":true},"thoughtSignature":"sig"}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.location","stringValue":"Paris","willContinue":true}],"willContinue":true}}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.unit","stringValue":"C","willContinue":true}],"willContinue":true}}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.location"},{"jsonPath":"$.unit"}],"willContinue":true}}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3}}

"#,
        );
        bridge.finish();
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");
        let completed = bridge.completed().expect("partial tool should complete");
        assert!(
            output
                .matches("response.function_call_arguments.delta")
                .count()
                >= 2
        );
        assert_eq!(
            completed.response_body["output"][0]["arguments"],
            r#"{"location":"Paris","unit":"C"}"#
        );
        let native_tool_part = &completed.continuation.native_turns[&1][0];
        assert_eq!(native_tool_part["thoughtSignature"], "sig");
        assert_eq!(
            native_tool_part["functionCall"]["args"],
            json!({"location": "Paris", "unit": "C"})
        );
    }
}

use super::contracts::MessagesBridgeResponse;
use serde_json::Value;

mod frame;
mod gemini;
mod messages;

pub use gemini::GeminiStreamBridge;
pub use messages::MessagesStreamBridge;

pub enum AdapterStreamBridge {
    Messages(Box<MessagesStreamBridge>),
    Gemini(Box<GeminiStreamBridge>),
    Translated(Box<super::translation::TranslationStream>),
}

impl AdapterStreamBridge {
    pub fn push(&mut self, bytes: &[u8]) {
        match self {
            Self::Messages(bridge) => bridge.push(bytes),
            Self::Gemini(bridge) => bridge.push(bytes),
            Self::Translated(bridge) => bridge.push(bytes),
        }
    }
    pub fn finish(&mut self) {
        match self {
            Self::Messages(bridge) => bridge.finish(),
            Self::Gemini(bridge) => bridge.finish(),
            Self::Translated(bridge) => bridge.finish(),
        }
    }
    pub fn pop_output(&mut self) -> Option<Vec<u8>> {
        match self {
            Self::Messages(bridge) => bridge.pop_output(),
            Self::Gemini(bridge) => bridge.pop_output(),
            Self::Translated(bridge) => bridge.pop_output(),
        }
    }
    pub fn completed(&self) -> Option<&MessagesBridgeResponse> {
        match self {
            Self::Messages(bridge) => bridge.completed(),
            Self::Gemini(bridge) => bridge.completed(),
            Self::Translated(bridge) => bridge.completed(),
        }
    }
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Messages(bridge) => bridge.is_terminal(),
            Self::Gemini(bridge) => bridge.is_terminal(),
            Self::Translated(bridge) => bridge.is_terminal(),
        }
    }
    pub fn take_upstream_error(&mut self) -> Option<Value> {
        match self {
            Self::Messages(bridge) => bridge.take_upstream_error(),
            Self::Gemini(bridge) => bridge.take_upstream_error(),
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
        let mut bridge = GeminiStreamBridge::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}

data: {"candidates":[{"content":{"parts":[{"text":"Hello world"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5}}

"#,
        );
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
        let mut bridge = GeminiStreamBridge::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"run","args":{"command":"pwd"},"id":"call-1"}}]}}]}

data: {"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":2}}

"#,
        );
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
        assert_eq!(bridge.completed().unwrap().continuation.messages.len(), 2);
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
        let mut bridge = GeminiStreamBridge::new(request);
        bridge.push(
            br#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","willContinue":true},"thoughtSignature":"sig"}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.location","stringValue":"Paris","willContinue":true}],"willContinue":true}}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.unit","stringValue":"C","willContinue":true}],"willContinue":true}}]}}]}

data: {"candidates":[{"content":{"parts":[{"functionCall":{}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3}}

"#,
        );
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
        assert_eq!(
            completed.continuation.messages[1]["parts"][0]["functionCall"]["thoughtSignature"],
            "sig"
        );
    }
}

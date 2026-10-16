use crate::runtime::ExecutorRoute;
use crate::{GatewayRuntime, WireApi};
use serde_json::Value;
use std::collections::BTreeSet;

/// Only call identities are retained here. Payloads and provider state remain
/// in the existing bounded conversation stores.
#[derive(Default)]
pub(in crate::gateway) struct ToolCallCapture {
    ids: BTreeSet<String>,
}

impl ToolCallCapture {
    pub(in crate::gateway) fn observe(&mut self, body: &Value, client: WireApi) {
        let ids: Vec<&str> = match client {
            WireApi::Responses => {
                let response = body.get("response").unwrap_or(body);
                response
                    .get("output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .chain(body.get("item"))
                    .filter(|item| {
                        matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("function_call" | "custom_tool_call")
                        )
                    })
                    .filter_map(|item| item.get("call_id").and_then(Value::as_str))
                    .collect()
            }
            WireApi::ChatCompletions => body
                .get("choices")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|choice| choice.get("message").or_else(|| choice.get("delta")))
                .flat_map(|message| {
                    message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(|call| call.get("id").and_then(Value::as_str))
                .collect(),
            WireApi::Messages => body
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .chain(body.get("content_block"))
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
                .filter_map(|block| block.get("id").and_then(Value::as_str))
                .collect(),
            WireApi::Gemini => body
                .get("candidates")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|candidate| {
                    candidate
                        .pointer("/content/parts")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(|part| part.pointer("/functionCall/id").and_then(Value::as_str))
                .collect(),
        };
        for id in ids {
            if self.ids.len() >= 1_024 {
                break;
            }
            if !id.is_empty() && id.len() <= 512 {
                self.ids.insert(id.to_owned());
            }
        }
    }

    pub(in crate::gateway) fn bind(
        &self,
        runtime: &GatewayRuntime,
        local_key_id: &str,
        route: &ExecutorRoute,
        now_ms: u64,
    ) {
        for id in &self.ids {
            runtime.bind_tool_call_route_affinity(local_key_id, id, route, now_ms);
        }
    }
}

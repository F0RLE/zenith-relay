use super::super::super::super::contracts::{custom_tool_item_id, AdapterError, ResponsesToolKind};
use super::super::{GeminiStreamBridge, GeminiStreamCall};
use serde_json::{json, Map, Value};

impl GeminiStreamBridge {
    pub(super) fn handle_function_call(&mut self, part: &Map<String, Value>) {
        let (key, is_new) = match self.function_calls.push(&Value::Object(part.clone())) {
            Ok(result) => result,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        let state = self.function_calls.get(key);
        let name = state.name.clone();
        let native_part = state.native_part();
        if is_new {
            let Some(target) = self.request.bridge_state().client_tool(&name) else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            let kind = target.kind;
            let target_name = target.name.clone();
            let namespace = target.namespace.clone();
            let id = state
                .provider_id
                .clone()
                .unwrap_or_else(|| format!("call_{}_{}", self.request.response_id(), key));
            if self.calls.values().any(|call| call.id == id)
                || self
                    .request
                    .bridge_state()
                    .tool_call_targets
                    .contains_key(&id)
            {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            }
            let item_id = if kind == ResponsesToolKind::Custom {
                custom_tool_item_id(&id)
            } else {
                id.clone()
            };
            self.ensure_started();
            self.active_text = None;
            let output_index = self.next_output_index;
            self.next_output_index = self.next_output_index.saturating_add(1);
            let native_part_index = self.native_parts.len();
            self.native_parts.push(native_part);
            self.calls.insert(
                key,
                GeminiStreamCall {
                    id: id.clone(),
                    native_part_index,
                },
            );
            let mut output_item = if kind == ResponsesToolKind::Custom {
                json!({"id":item_id,"type":"custom_tool_call","status":"in_progress","call_id":id,"name":target_name,"input":""})
            } else {
                json!({"id":item_id,"type":"function_call","status":"in_progress","call_id":id,"name":target_name,"arguments":""})
            };
            if let Some(namespace) = namespace {
                output_item["namespace"] = namespace.into();
            }
            self.frame("response.output_item.added", json!({"type":"response.output_item.added","output_index":output_index,"item":output_item}));
        } else if let Some(call) = self.calls.get(&key) {
            self.native_parts[call.native_part_index] = native_part;
        } else {
            self.fail(AdapterError::upstream_stream_invalid());
        }
    }
}

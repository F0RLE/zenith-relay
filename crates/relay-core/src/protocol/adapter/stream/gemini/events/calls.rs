use super::super::super::super::contracts::{custom_tool_item_id, AdapterError, ResponsesToolKind};
use super::super::super::super::gemini::{apply_partial_args, function_call_args};
use super::super::super::frame::incremental_delta;
use super::super::{GeminiStreamBridge, GeminiStreamCall, GeminiStreamOutput};
use serde_json::{json, Map, Value};

impl GeminiStreamBridge {
    pub(super) fn handle_function_call(
        &mut self,
        part_index: usize,
        part: &Map<String, Value>,
        call: &Map<String, Value>,
    ) {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let Some(key) = self.function_call_key(part_index, name, call_id) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let name = name
            .map(str::to_string)
            .or_else(|| self.calls.get(&key).map(|state| state.name.clone()));
        let Some(name) = name else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(target) = self.request.state().client_tool(&name) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let target_kind = target.kind;
        let target_name = target.name.clone();
        let target_namespace = target.namespace.clone();
        let is_new = !self.calls.contains_key(&key);
        if is_new {
            let args = match function_call_args(call) {
                Ok(args) => args,
                Err(_) => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            };
            self.ensure_started();
            let output_index = self.next_output_index;
            self.next_output_index = self.next_output_index.saturating_add(1);
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{}_{}", self.request.response_id(), key));
            let item_id = if target_kind == ResponsesToolKind::Custom {
                custom_tool_item_id(&id)
            } else {
                id.clone()
            };
            self.calls.insert(
                key,
                GeminiStreamCall {
                    id,
                    item_id,
                    name,
                    kind: target_kind,
                    args,
                    output_index,
                    thought_signature: part
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    emitted_arguments: String::new(),
                },
            );
            self.order.push(GeminiStreamOutput::Call(key));
        }
        let args = if is_new {
            self.calls
                .get(&key)
                .map(|state| state.args.clone())
                .unwrap_or_else(|| json!({}))
        } else {
            let mut args = self
                .calls
                .get(&key)
                .map(|state| state.args.clone())
                .unwrap_or_else(|| json!({}));
            if let Some(full_args) = call.get("args") {
                if !full_args.is_object() {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
                args = full_args.clone();
            }
            if let Some(partial_args) = call.get("partialArgs") {
                if apply_partial_args(&mut args, partial_args).is_err() {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            }
            args
        };
        if let Some(call_state) = self.calls.get_mut(&key) {
            call_state.args = args.clone();
            if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
                call_state.thought_signature = Some(signature.to_string());
            }
        }
        if is_new {
            let call_state = self.calls.get(&key).expect("inserted Gemini call").clone();
            let mut item = if call_state.kind == ResponsesToolKind::Custom {
                json!({"id":call_state.item_id,"type":"custom_tool_call","status":"in_progress","call_id":call_state.id,"name":target_name,"input":""})
            } else {
                json!({"id":call_state.id,"type":"function_call","status":"in_progress","call_id":call_state.id,"name":target_name,"arguments":""})
            };
            if let Some(namespace) = target_namespace {
                item["namespace"] = Value::String(namespace);
            }
            self.frame("response.output_item.added", json!({"type":"response.output_item.added","output_index":call_state.output_index,"item":item}));
            if call_state.kind == ResponsesToolKind::Function {
                self.emit_call_arguments_delta(key);
            }
        }
        if let Some(call_state) = self.calls.get(&key) {
            if !is_new && call_state.kind == ResponsesToolKind::Function {
                self.emit_call_arguments_delta(key);
            }
        }
        if call.get("willContinue").and_then(Value::as_bool) == Some(true) {
            self.active_call = Some(key);
        } else if self.active_call == Some(key) {
            self.active_call = None;
        }
    }

    fn function_call_key(
        &self,
        part_index: usize,
        name: Option<&str>,
        call_id: Option<&str>,
    ) -> Option<usize> {
        if let Some(call_id) = call_id {
            self.calls
                .iter()
                .find_map(|(key, state)| (state.id == call_id).then_some(*key))
        } else {
            None
        }
        .or_else(|| {
            name.and_then(|name| {
                self.calls.iter().find_map(|(key, state)| {
                    (state.name == name && *key == part_index).then_some(*key)
                })
            })
        })
        .or_else(|| {
            self.active_call.filter(|active| {
                name.is_none()
                    || self
                        .calls
                        .get(active)
                        .is_some_and(|state| Some(state.name.as_str()) == name)
            })
        })
        .or_else(|| {
            name.map(|_| {
                if self.calls.contains_key(&part_index) {
                    self.calls
                        .keys()
                        .next_back()
                        .copied()
                        .unwrap_or(part_index)
                        .saturating_add(1)
                } else {
                    part_index
                }
            })
        })
    }

    fn emit_call_arguments_delta(&mut self, key: usize) {
        let Some(call_state) = self.calls.get_mut(&key) else {
            return;
        };
        let Ok(arguments) = serde_json::to_string(&call_state.args) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if call_state.emitted_arguments.is_empty()
            && call_state.args.as_object().is_some_and(Map::is_empty)
        {
            return;
        }
        let delta = incremental_delta(&call_state.emitted_arguments, &arguments);
        if delta.is_empty() {
            return;
        }
        call_state.emitted_arguments = arguments;
        let item_id = call_state.item_id.clone();
        let output_index = call_state.output_index;
        let response_id = self.request.response_id().to_string();
        self.frame(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","response_id":response_id,"item_id":item_id,"output_index":output_index,"delta":delta}),
        );
    }
}

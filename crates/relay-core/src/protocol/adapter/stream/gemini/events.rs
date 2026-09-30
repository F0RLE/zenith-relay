mod calls;

use super::super::super::contracts::{AdapterError, MessagesBridgeResponse};
use super::super::frame::{
    failed_responses_event, incremental_delta, parse_sse_data, push_sse_frame, sse_done,
    sse_event_has_data,
};
use super::{GeminiStreamBridge, GeminiStreamOutput};
use serde_json::{json, Value};

impl GeminiStreamBridge {
    pub(super) fn handle_event(&mut self, event: &[u8]) {
        if sse_done(event) {
            if self.finished_upstream {
                self.complete();
            } else {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        }
        let Some(value) = parse_sse_data(event) else {
            if sse_event_has_data(event) {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        if value.get("error").is_some() {
            self.upstream_error = Some(value);
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if let Some(usage) = value.get("usageMetadata") {
            self.usage = Some(usage.clone());
        }
        match super::super::super::gemini::prompt_blocked(&value) {
            Ok(true) => {
                self.complete_prompt_block(&value);
                return;
            }
            Err(()) => {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            }
            Ok(false) => {}
        }
        let Some(candidate) = value
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return;
        };
        let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        else {
            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_string());
                self.finished_upstream = true;
                self.complete();
            } else {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        for (part_index, part) in parts.iter().enumerate() {
            let Some(part) = part.as_object() else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            if let Some(value) = part.get("text").and_then(Value::as_str) {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    self.append_thought_text(value);
                } else {
                    self.append_output_text(value);
                }
                if self.terminal {
                    return;
                }
                continue;
            }
            if let Some(call) = part.get("functionCall").and_then(Value::as_object) {
                self.handle_function_call(part_index, part, call);
                if self.terminal {
                    return;
                }
                continue;
            }
            if part.get("thoughtSignature").is_some() {
                if let Some(call) = self.calls.values_mut().next_back() {
                    call.thought_signature = part
                        .get("thoughtSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                continue;
            }
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(|reason| {
                self.finish_reason = Some(reason.to_string());
                true
            })
            .unwrap_or(false)
        {
            self.finished_upstream = true;
            self.complete();
        }
    }

    fn complete_prompt_block(&mut self, upstream: &Value) {
        // A blocked prompt has no model output. Do not turn a partial stream
        // that already emitted data into a successful filtered response.
        if self.started
            || !self.text.is_empty()
            || !self.thinking.is_empty()
            || !self.calls.is_empty()
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let mut upstream = upstream.clone();
        if upstream.get("usageMetadata").is_none() {
            if let Some(usage) = &self.usage {
                upstream["usageMetadata"] = usage.clone();
            }
        }
        let response = match super::super::super::gemini::translate_gemini_response(
            self.request.clone(),
            &upstream,
        ) {
            Ok(response) => response,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        self.ensure_started();
        self.frame(
            "response.incomplete",
            json!({"type":"response.incomplete","response":response.response_body}),
        );
        self.completed = Some(MessagesBridgeResponse {
            response_body: response.response_body,
            response_id: response.response_id,
            continuation: response.continuation,
        });
        self.terminal = true;
    }

    fn append_thought_text(&mut self, value: &str) {
        self.ensure_started();
        if self.terminal {
            return;
        }
        let delta = incremental_delta(&self.thinking, value);
        if !delta.is_empty() {
            if self.thinking.is_empty() {
                self.order.push(GeminiStreamOutput::Thinking);
                self.thinking_output_index = Some(self.next_output_index);
                self.next_output_index = self.next_output_index.saturating_add(1);
                self.frame(
                    "response.output_item.added",
                    json!({"type":"response.output_item.added","output_index":self.thinking_output_index,"item":{"id":format!("reasoning_{}_0",self.request.response_id()),"type":"reasoning","status":"in_progress","summary":[]}}),
                );
            }
            self.thinking.push_str(&delta);
            self.frame(
                "response.reasoning_summary_text.delta",
                json!({
                    "type": "response.reasoning_summary_text.delta",
                    "response_id": self.request.response_id(),
                    "output_index": self.thinking_output_index,
                    "delta": delta,
                }),
            );
        }
    }

    fn append_output_text(&mut self, value: &str) {
        self.ensure_started();
        if self.terminal {
            return;
        }
        let delta = incremental_delta(&self.text, value);
        if !delta.is_empty() {
            if self.text.is_empty() {
                self.order.push(GeminiStreamOutput::Text);
            }
            self.ensure_text_output();
            if self.terminal {
                return;
            }
            self.text.push_str(&delta);
            self.frame(
                "response.output_text.delta",
                json!({
                    "type": "response.output_text.delta",
                    "response_id": self.request.response_id(),
                    "item_id": format!("msg_{}_{}", self.request.response_id(), self.text_output_index),
                    "output_index": self.text_output_index,
                    "content_index": 0,
                    "delta": delta,
                }),
            );
        }
    }

    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        self.frame(
            "response.created",
            json!({
                "type": "response.created",
                "response": {
                    "id": self.request.response_id(),
                    "object": "response",
                    "status": "in_progress",
                    "model": self.request.model(),
                    "output": [],
                }
            }),
        );
    }

    fn ensure_text_output(&mut self) {
        if self.text_started {
            return;
        }
        self.text_started = true;
        self.text_output_index = self.next_output_index;
        self.next_output_index = self.next_output_index.saturating_add(1);
        let item_id = format!(
            "msg_{}_{}",
            self.request.response_id(),
            self.text_output_index
        );
        self.frame(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":self.text_output_index,"item":{
                "id": item_id, "type":"message",
                "status":"in_progress","role":"assistant","content":[]
            }}),
        );
        self.frame(
            "response.content_part.added",
            json!({"type":"response.content_part.added","item_id":format!("msg_{}_{}", self.request.response_id(), self.text_output_index),
                "output_index":self.text_output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        );
    }

    fn frame(&mut self, event: &str, payload: Value) {
        if !push_sse_frame(&mut self.output, event, &payload) {
            self.terminal = true;
        }
    }

    pub(in crate::protocol::adapter::stream::gemini) fn complete(&mut self) {
        if self.terminal {
            return;
        }
        let incomplete = match super::super::super::gemini::candidate_incomplete_reason(
            self.finish_reason.as_deref(),
        ) {
            Ok(reason) => reason.is_some(),
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        if self.text.is_empty() && self.thinking.is_empty() && self.calls.is_empty() && !incomplete
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        self.ensure_started();
        let mut parts = Vec::new();
        for item in &self.order {
            match item {
                GeminiStreamOutput::Thinking if !self.thinking.is_empty() => {
                    parts.push(json!({"thought":true,"text":self.thinking}));
                }
                GeminiStreamOutput::Text if !self.text.is_empty() => {
                    parts.push(json!({"text":self.text}));
                }
                GeminiStreamOutput::Call(key) => {
                    if let Some(call) = self.calls.get(key) {
                        let mut function_call =
                            json!({"name":call.name,"args":call.args,"id":call.id});
                        if let Some(signature) = call.thought_signature.as_ref() {
                            function_call["thoughtSignature"] = Value::String(signature.clone());
                        }
                        parts.push(json!({"functionCall":function_call}));
                    }
                }
                _ => {}
            }
        }
        let upstream = json!({"candidates":[{"content":{"parts":parts},"finishReason":self.finish_reason}],"usageMetadata":self.usage.clone()});
        let response = match super::super::super::gemini::translate_gemini_response(
            self.request.clone(),
            &upstream,
        ) {
            Ok(response) => response,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        for (output_index, item) in response.response_body["output"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            match item.get("type").and_then(Value::as_str) {
                Some("function_call") => self.frame(
                    "response.function_call_arguments.done",
                    json!({"type":"response.function_call_arguments.done","response_id":self.request.response_id(),"item_id":item["id"],"call_id":item["call_id"],"name":item["name"],"output_index":output_index,"arguments":item["arguments"]}),
                ),
                Some("custom_tool_call") => self.frame(
                    "response.custom_tool_call_input.done",
                    json!({"type":"response.custom_tool_call_input.done","response_id":self.request.response_id(),"item_id":item["id"],"output_index":output_index,"input":item["input"]}),
                ),
                Some("message") => {
                    self.frame(
                        "response.output_text.done",
                        json!({"type":"response.output_text.done","response_id":self.request.response_id(),"item_id":item["id"],"output_index":output_index,"content_index":0,"text":item["content"][0]["text"]}),
                    );
                    self.frame(
                        "response.content_part.done",
                        json!({"type":"response.content_part.done","response_id":self.request.response_id(),"item_id":item["id"],"output_index":output_index,"content_index":0}),
                    );
                }
                _ => {}
            }
            self.frame("response.output_item.done", json!({"type":"response.output_item.done","response_id":self.request.response_id(),"output_index":output_index,"item":item}));
        }
        self.completed = Some(MessagesBridgeResponse {
            response_body: response.response_body.clone(),
            response_id: response.response_id.clone(),
            continuation: response.continuation,
        });
        let kind = if incomplete {
            "response.incomplete"
        } else {
            "response.completed"
        };
        self.frame(
            kind,
            json!({"type": kind, "response": response.response_body}),
        );
        self.terminal = true;
    }

    pub(in crate::protocol::adapter::stream::gemini) fn fail(&mut self, error: AdapterError) {
        if self.terminal {
            return;
        }
        self.frame(
            "response.failed",
            failed_responses_event(
                self.request.response_id(),
                self.request.model(),
                error.code(),
                error.message(),
            ),
        );
        self.terminal = true;
    }
}

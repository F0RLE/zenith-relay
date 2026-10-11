mod calls;

use super::super::super::contracts::{AdapterError, MessagesBridgeResponse};
use super::super::frame::{
    failed_responses_event, parse_sse_data, push_sse_frame, sse_done, sse_event_has_data,
};
use super::GeminiStreamBridge;
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
        let Some(event_payload) = parse_sse_data(event) else {
            if sse_event_has_data(event) {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        if let Some(usage) = event_payload.get("usageMetadata") {
            super::super::frame::merge_usage(&mut self.usage, usage);
        }
        if event_payload
            .get("error")
            .is_some_and(|error| !error.is_null())
        {
            self.upstream_error = Some(event_payload);
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if self.completion.observe(&event_payload).is_err() {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if self.finished_upstream {
            if event_payload
                .get("candidates")
                .is_none_or(|candidates| candidates.as_array().is_some_and(Vec::is_empty))
                && event_payload.get("promptFeedback").is_none()
                && event_payload
                    .get("usageMetadata")
                    .is_some_and(Value::is_object)
            {
                return;
            }
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        match super::super::super::gemini::prompt_blocked(&event_payload) {
            Ok(true) => {
                if self.started || !self.native_parts.is_empty() {
                    self.fail(AdapterError::upstream_stream_invalid());
                } else {
                    self.prompt_response = Some(event_payload);
                    self.finished_upstream = true;
                }
                return;
            }
            Err(()) => {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            }
            Ok(false) => {}
        }
        let Some(candidates) = event_payload.get("candidates").and_then(Value::as_array) else {
            if !event_payload
                .get("usageMetadata")
                .is_some_and(Value::is_object)
            {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        if candidates.is_empty()
            && event_payload
                .get("usageMetadata")
                .is_some_and(Value::is_object)
        {
            return;
        }
        if candidates.len() != 1
            || candidates[0]
                .get("index")
                .is_some_and(|index| index.as_u64() != Some(0))
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let candidate = &candidates[0];
        let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        else {
            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_string());
                self.finished_upstream = true;
            } else {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        for part in parts {
            let part = match super::super::super::gemini::validate_response_part(part) {
                Ok(part) => part,
                Err(_) => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            };
            if let Some(text_delta) = part.get("text").and_then(Value::as_str) {
                self.native_parts.push(Value::Object(part.clone()));
                self.append_text(
                    text_delta,
                    part.get("thought").and_then(Value::as_bool) == Some(true),
                );
                if self.terminal {
                    return;
                }
                continue;
            }
            if part.get("functionCall").is_some() {
                self.handle_function_call(part);
                if self.terminal {
                    return;
                }
                continue;
            }
            if part.get("thoughtSignature").is_some() {
                self.native_parts.push(Value::Object(part.clone()));
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
        }
    }

    fn complete_prompt_block(&mut self, upstream_event: &Value) {
        // A blocked prompt has no model output. Do not turn a partial stream
        // that already emitted data into a successful filtered response.
        if self.started || !self.native_parts.is_empty() {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let mut prompt_response_payload = upstream_event.clone();
        if let Some(usage) = &self.usage {
            prompt_response_payload["usageMetadata"] = usage.clone();
        }
        let translated_response = match super::super::super::gemini::translate_gemini_response(
            self.request.clone(),
            &prompt_response_payload,
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
            json!({"type":"response.incomplete","response":translated_response.response_body}),
        );
        self.completed = Some(MessagesBridgeResponse {
            response_body: translated_response.response_body,
            response_id: translated_response.response_id,
            continuation: translated_response.continuation,
        });
        self.terminal = true;
    }

    fn append_text(&mut self, text_delta: &str, reasoning: bool) {
        if text_delta.is_empty() {
            return;
        }
        self.ensure_started();
        if self.terminal {
            return;
        }
        let output_index = match self.active_text {
            Some((previous_reasoning, output_index)) if previous_reasoning == reasoning => {
                output_index
            }
            _ => {
                let output_index = self.next_output_index;
                self.next_output_index = self.next_output_index.saturating_add(1);
                self.active_text = Some((reasoning, output_index));
                let item_id = self.text_item_id(reasoning, output_index);
                let item = if reasoning {
                    json!({"id":item_id,"type":"reasoning","status":"in_progress","summary":[]})
                } else {
                    json!({"id":item_id,"type":"message","status":"in_progress","role":"assistant","content":[]})
                };
                self.frame("response.output_item.added", json!({"type":"response.output_item.added","output_index":output_index,"item":item}));
                if reasoning {
                    self.frame("response.reasoning_summary_part.added", json!({"type":"response.reasoning_summary_part.added","item_id":item_id,"output_index":output_index,"summary_index":0,"part":{"type":"summary_text","text":""}}));
                } else {
                    self.frame("response.content_part.added", json!({"type":"response.content_part.added","item_id":item_id,"output_index":output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}));
                }
                output_index
            }
        };
        let item_id = self.text_item_id(reasoning, output_index);
        if reasoning {
            self.frame(
                "response.reasoning_summary_text.delta",
                json!({"type":"response.reasoning_summary_text.delta","response_id":self.request.response_id(),"item_id":item_id,"output_index":output_index,"summary_index":0,"delta":text_delta}),
            );
        } else {
            self.frame(
                "response.output_text.delta",
                json!({
                    "type": "response.output_text.delta",
                    "response_id": self.request.response_id(),
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": text_delta,
                }),
            );
        }
    }

    fn text_item_id(&self, reasoning: bool, output_index: usize) -> String {
        let prefix = if reasoning { "reasoning" } else { "msg" };
        format!("{prefix}_{}_{output_index}", self.request.response_id())
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

    fn frame(&mut self, event: &str, mut event_payload: Value) {
        event_payload["sequence_number"] = self.sequence_number.into();
        self.sequence_number = self.sequence_number.saturating_add(1);
        if !push_sse_frame(&mut self.output, event, &event_payload) {
            self.terminal = true;
        }
    }

    pub(in crate::protocol::adapter::stream::gemini) fn complete(&mut self) {
        if self.terminal {
            return;
        }
        if self.completion.outcome() == crate::protocol::completion::ResponseCompletion::Invalid {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if let Some(prompt_response) = self.prompt_response.take() {
            self.complete_prompt_block(&prompt_response);
            return;
        }
        if self.function_calls.has_open_calls() {
            self.fail(AdapterError::upstream_stream_invalid());
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
        if self.next_output_index == 0 && !incomplete {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        self.ensure_started();
        let parts = self.native_parts.clone();
        let client_call_ids = self
            .calls
            .values()
            .map(|call| (call.native_part_index, call.id.clone()))
            .collect();
        let upstream_response = json!({"candidates":[{"content":{"parts":parts},"finishReason":self.finish_reason}],"usageMetadata":self.usage.clone()});
        let translated_response =
            match super::super::super::gemini::translate_gemini_response_with_call_ids(
                self.request.clone(),
                &upstream_response,
                &client_call_ids,
            ) {
                Ok(response) => response,
                Err(error) => {
                    self.fail(error);
                    return;
                }
            };
        for (output_index, output_item) in translated_response.response_body["output"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            match output_item.get("type").and_then(Value::as_str) {
                Some("function_call") => {
                    // Gemini streams typed argument snapshots and JSON-path
                    // updates, not append-only JSON strings. Emit one valid
                    // argument delta after assembling the complete object.
                    self.frame(
                        "response.function_call_arguments.delta",
                        json!({"type":"response.function_call_arguments.delta","response_id":self.request.response_id(),"item_id":output_item["id"],"output_index":output_index,"delta":output_item["arguments"]}),
                    );
                    self.frame(
                        "response.function_call_arguments.done",
                        json!({"type":"response.function_call_arguments.done","response_id":self.request.response_id(),"item_id":output_item["id"],"call_id":output_item["call_id"],"name":output_item["name"],"output_index":output_index,"arguments":output_item["arguments"]}),
                    );
                }
                Some("custom_tool_call") => {
                    self.frame(
                        "response.custom_tool_call_input.delta",
                        json!({"type":"response.custom_tool_call_input.delta","item_id":output_item["id"],"output_index":output_index,"delta":output_item["input"]}),
                    );
                    self.frame(
                    "response.custom_tool_call_input.done",
                    json!({"type":"response.custom_tool_call_input.done","response_id":self.request.response_id(),"item_id":output_item["id"],"output_index":output_index,"input":output_item["input"]}),
                    );
                }
                Some("message") => {
                    self.frame(
                        "response.output_text.done",
                        json!({"type":"response.output_text.done","response_id":self.request.response_id(),"item_id":output_item["id"],"output_index":output_index,"content_index":0,"text":output_item["content"][0]["text"]}),
                    );
                    self.frame(
                        "response.content_part.done",
                        json!({"type":"response.content_part.done","response_id":self.request.response_id(),"item_id":output_item["id"],"output_index":output_index,"content_index":0,"part":output_item["content"][0]}),
                    );
                }
                Some("reasoning") => {
                    self.frame(
                        "response.reasoning_summary_text.done",
                        json!({"type":"response.reasoning_summary_text.done","item_id":output_item["id"],"output_index":output_index,"summary_index":0,"text":output_item["summary"][0]["text"]}),
                    );
                    self.frame(
                        "response.reasoning_summary_part.done",
                        json!({"type":"response.reasoning_summary_part.done","item_id":output_item["id"],"output_index":output_index,"summary_index":0,"part":output_item["summary"][0]}),
                    );
                }
                _ => {}
            }
            self.frame("response.output_item.done", json!({"type":"response.output_item.done","response_id":self.request.response_id(),"output_index":output_index,"item":output_item}));
        }
        self.completed = Some(MessagesBridgeResponse {
            response_body: translated_response.response_body.clone(),
            response_id: translated_response.response_id.clone(),
            continuation: translated_response.continuation,
        });
        let kind = if incomplete {
            "response.incomplete"
        } else {
            "response.completed"
        };
        self.frame(
            kind,
            json!({"type": kind, "response": translated_response.response_body}),
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

use crate::WireApi;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Receiving a JSON body or an HTTP 200 does not prove a completed generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResponseCompletion {
    Completed,
    Incomplete,
    Invalid,
}

impl ResponseCompletion {
    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Incomplete)
    }

    pub(crate) fn combine(self, next: Self) -> Self {
        match (self, next) {
            (Self::Invalid, _) | (_, Self::Invalid) => Self::Invalid,
            (Self::Incomplete, _) | (_, Self::Incomplete) => Self::Incomplete,
            _ => Self::Completed,
        }
    }

    pub(crate) fn from_responses_event(payload: &Value) -> Option<Self> {
        let event_type = payload.get("type").and_then(Value::as_str)?;
        if !matches!(
            event_type,
            "response.completed" | "response.done" | "response.incomplete"
        ) {
            return None;
        }
        let completion = Self::for_protocol(WireApi::Responses, payload);
        Some(
            if matches!(
                (event_type, completion),
                ("response.incomplete", Self::Completed) | ("response.completed", Self::Incomplete)
            ) {
                Self::Invalid
            } else {
                completion
            },
        )
    }

    fn from_chat_finish(reason: &str) -> Self {
        match reason {
            "stop" | "tool_calls" | "function_call" => Self::Completed,
            "length" | "content_filter" => Self::Incomplete,
            _ => Self::Invalid,
        }
    }

    fn from_messages_stop(reason: &str) -> Self {
        match reason {
            "end_turn" | "stop_sequence" | "tool_use" | "refusal" => Self::Completed,
            "max_tokens" | "model_context_window_exceeded" | "pause_turn" => Self::Incomplete,
            _ => Self::Invalid,
        }
    }

    pub(crate) fn from_body(body: &Value) -> Self {
        let body = body.get("response").unwrap_or(body);
        let protocol = if body.get("outputs").is_some() || body.get("interaction").is_some() {
            WireApi::GeminiInteractions
        } else if body.get("choices").is_some() {
            WireApi::ChatCompletions
        } else if body.get("candidates").is_some() || body.get("promptFeedback").is_some() {
            WireApi::Gemini
        } else if body.get("stop_reason").is_some() {
            WireApi::Messages
        } else {
            WireApi::Responses
        };
        Self::for_protocol(protocol, body)
    }

    pub(crate) fn for_protocol(protocol: WireApi, body: &Value) -> Self {
        let body = body.get("response").unwrap_or(body);
        if !body.is_object() || body.get("error").is_some_and(|error| !error.is_null()) {
            return Self::Invalid;
        }
        match protocol {
            WireApi::GeminiInteractions => crate::protocol::interactions::completion(body),
            WireApi::Responses => {
                if !body.get("output").is_some_and(Value::is_array) {
                    return Self::Invalid;
                }
                match body.get("status").and_then(Value::as_str) {
                    Some("completed") => Self::Completed,
                    Some("incomplete") => Self::Incomplete,
                    _ => Self::Invalid,
                }
            }
            WireApi::ChatCompletions => {
                let Some(choices) = body.get("choices").and_then(Value::as_array) else {
                    return Self::Invalid;
                };
                if choices.is_empty() {
                    return Self::Invalid;
                }
                choices.iter().fold(Self::Completed, |outcome, choice| {
                    if !choice.get("message").is_some_and(Value::is_object) {
                        return Self::Invalid;
                    }
                    let current = choice
                        .get("finish_reason")
                        .and_then(Value::as_str)
                        .map(Self::from_chat_finish)
                        .unwrap_or(Self::Invalid);
                    outcome.combine(current)
                })
            }
            WireApi::Messages => {
                if !body.get("content").is_some_and(Value::is_array) {
                    return Self::Invalid;
                }
                body.get("stop_reason")
                    .and_then(Value::as_str)
                    .map(Self::from_messages_stop)
                    .unwrap_or(Self::Invalid)
            }
            WireApi::Gemini => {
                let mut completion = GeminiStreamCompletion::default();
                if completion.observe(body).is_err() {
                    return Self::Invalid;
                }
                completion.outcome()
            }
        }
    }
}

#[derive(Debug, Default)]
struct GeminiCandidateCompletion {
    outcome: Option<ResponseCompletion>,
    finish_reason: Option<String>,
    has_content: bool,
}

/// Native Gemini can stream several candidates and send usage after they finish.
/// Each observed candidate must finish before EOF can complete the request.
#[derive(Debug, Default)]
pub(crate) struct GeminiStreamCompletion {
    candidates: BTreeMap<u64, GeminiCandidateCompletion>,
    prompt_blocked: bool,
}

impl GeminiStreamCompletion {
    pub(crate) fn observe(&mut self, payload: &Value) -> Result<(), ()> {
        let foreign_event = payload
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                kind.starts_with("response.")
                    || matches!(
                        kind,
                        "message_start"
                            | "message_delta"
                            | "message_stop"
                            | "content_block_start"
                            | "content_block_delta"
                            | "content_block_stop"
                            | "ping"
                    )
            });
        if !payload.is_object() || foreign_event || payload.get("choices").is_some() {
            return Err(());
        }
        if super::prompt_blocked(payload)? {
            if !self.candidates.is_empty() {
                return Err(());
            }
            self.prompt_blocked = true;
        }
        let Some(candidates) = payload.get("candidates") else {
            return Ok(());
        };
        let candidates = candidates.as_array().ok_or(())?;
        if self.prompt_blocked && !candidates.is_empty() {
            return Err(());
        }
        let mut event_indices = BTreeSet::new();
        for candidate in candidates {
            if !candidate.is_object() {
                return Err(());
            }
            let index = match candidate.get("index") {
                Some(index) => index.as_u64().ok_or(())?,
                None => 0,
            };
            if !event_indices.insert(index) {
                return Err(());
            }
            let state = self.candidates.entry(index).or_default();
            // A candidate is allowed to appear in many deltas, but once its
            // finishReason has been emitted it must never be reopened. Usage
            // metadata is carried by an event without candidates.
            if state.outcome.is_some() {
                return Err(());
            }
            if let Some(content) = candidate.get("content") {
                let parts = content.get("parts").and_then(Value::as_array).ok_or(())?;
                if content.get("role").is_some_and(|role| {
                    role.as_str()
                        .is_none_or(|role| !matches!(role, "" | "model"))
                }) || parts.iter().any(|part| !part.is_object())
                {
                    return Err(());
                }
                state.has_content |= !parts.is_empty();
            }
            if let Some(reason) = candidate
                .get("finishReason")
                .filter(|reason| !reason.is_null())
            {
                let reason = reason.as_str().ok_or(())?;
                if reason == "CONTINUATION"
                    && candidate
                        .get("continuationToken")
                        .and_then(Value::as_str)
                        .is_none_or(|token| token.is_empty())
                {
                    return Err(());
                }
                let outcome =
                    match super::candidate_incomplete_reason(Some(reason)).map_err(|_| ())? {
                        Some(_) => ResponseCompletion::Incomplete,
                        None if state.has_content => ResponseCompletion::Completed,
                        None => return Err(()),
                    };
                if state
                    .finish_reason
                    .as_deref()
                    .is_some_and(|previous| previous != reason)
                {
                    return Err(());
                }
                state.finish_reason = Some(reason.to_owned());
                state.outcome = Some(outcome);
            }
        }
        Ok(())
    }

    pub(crate) fn outcome(&self) -> ResponseCompletion {
        if self.prompt_blocked {
            return ResponseCompletion::Incomplete;
        }
        if self.candidates.is_empty() {
            return ResponseCompletion::Invalid;
        }
        self.candidates
            .values()
            .fold(ResponseCompletion::Completed, |outcome, candidate| {
                outcome.combine(candidate.outcome.unwrap_or(ResponseCompletion::Invalid))
            })
    }
}

/// A Chat transport marker closes the stream only after every choice finishes.
#[derive(Debug, Default)]
pub(crate) struct ChatStreamCompletion {
    choices: BTreeMap<u64, Option<String>>,
    usage_only: bool,
}

impl ChatStreamCompletion {
    pub(crate) fn observe(&mut self, payload: &Value) -> Result<(), ()> {
        let choices = payload.get("choices").and_then(Value::as_array).ok_or(())?;
        if self.usage_only {
            return Err(());
        }
        if choices.is_empty() {
            // OpenAI-compatible providers use one final usage-only chunk
            // after every choice has emitted a finish_reason. An empty
            // choices array before that point is an incomplete/invalid stream.
            if !payload.get("usage").is_some_and(Value::is_object) || !self.outcome().is_terminal()
            {
                return Err(());
            }
            self.usage_only = true;
            return Ok(());
        }
        let mut event_indices = BTreeSet::new();
        for choice in choices {
            let index = choice.get("index").and_then(Value::as_u64).ok_or(())?;
            if !event_indices.insert(index) {
                return Err(());
            }
            let previous_reason = self.choices.entry(index).or_default();
            if previous_reason.is_some() {
                return Err(());
            }
            if choice.get("delta").is_some_and(|delta| !delta.is_object()) {
                return Err(());
            }
            if let Some(reason) = choice
                .get("finish_reason")
                .filter(|reason| !reason.is_null())
            {
                let reason = reason.as_str().ok_or(())?;
                if ResponseCompletion::from_chat_finish(reason) == ResponseCompletion::Invalid
                    || previous_reason
                        .as_deref()
                        .is_some_and(|previous| previous != reason)
                {
                    return Err(());
                }
                *previous_reason = Some(reason.to_owned());
            }
        }
        Ok(())
    }

    pub(crate) fn outcome(&self) -> ResponseCompletion {
        if self.choices.is_empty() {
            return ResponseCompletion::Invalid;
        }
        self.choices
            .values()
            .fold(ResponseCompletion::Completed, |outcome, reason| {
                outcome.combine(
                    reason
                        .as_deref()
                        .map(ResponseCompletion::from_chat_finish)
                        .unwrap_or(ResponseCompletion::Invalid),
                )
            })
    }
}

/// Checks the Messages lifecycle while leaving native content extensions intact.
#[derive(Debug, Default)]
pub(crate) struct MessagesStreamCompletion {
    started: bool,
    blocks: BTreeMap<u64, bool>,
    stop_reason: Option<String>,
    stopped: bool,
}

impl MessagesStreamCompletion {
    pub(crate) fn observe(&mut self, payload: &Value) -> Result<Option<ResponseCompletion>, ()> {
        let kind = payload.get("type").and_then(Value::as_str).ok_or(())?;
        if self.stopped {
            return Err(());
        }
        match kind {
            "ping" => {}
            "error" => return Err(()),
            "message_start" => {
                let message = payload
                    .get("message")
                    .filter(|message| message.is_object())
                    .ok_or(())?;
                if self.started
                    || message
                        .get("id")
                        .and_then(Value::as_str)
                        .is_none_or(|id| id.trim().is_empty())
                    || message
                        .get("content")
                        .is_some_and(|content| !content.as_array().is_some_and(Vec::is_empty))
                    || message
                        .get("stop_reason")
                        .is_some_and(|reason| !reason.is_null())
                {
                    return Err(());
                }
                self.started = true;
            }
            "content_block_start" => {
                let index = payload.get("index").and_then(Value::as_u64).ok_or(())?;
                if !self.started
                    || self.stop_reason.is_some()
                    || !payload.get("content_block").is_some_and(Value::is_object)
                    || self.blocks.insert(index, false).is_some()
                {
                    return Err(());
                }
            }
            "content_block_delta" | "content_block_stop" => {
                let index = payload.get("index").and_then(Value::as_u64).ok_or(())?;
                let closed = self.blocks.get_mut(&index).ok_or(())?;
                if *closed || self.stop_reason.is_some() {
                    return Err(());
                }
                if kind == "content_block_stop" {
                    *closed = true;
                } else if !payload.get("delta").is_some_and(Value::is_object) {
                    return Err(());
                }
            }
            "message_delta" => {
                if !self.started || !payload.get("delta").is_some_and(Value::is_object) {
                    return Err(());
                }
                if let Some(reason) = payload
                    .pointer("/delta/stop_reason")
                    .filter(|reason| !reason.is_null())
                {
                    let reason = reason.as_str().ok_or(())?;
                    if self.stop_reason.is_some()
                        || self.blocks.values().any(|closed| !closed)
                        || ResponseCompletion::from_messages_stop(reason)
                            == ResponseCompletion::Invalid
                    {
                        return Err(());
                    }
                    self.stop_reason = Some(reason.to_owned());
                }
            }
            "message_stop" => {
                if !self.started || self.blocks.values().any(|closed| !closed) {
                    return Err(());
                }
                let reason = self.stop_reason.as_deref().ok_or(())?;
                self.stopped = true;
                return Ok(Some(ResponseCompletion::from_messages_stop(reason)));
            }
            _ if kind.starts_with("response.")
                || payload.get("choices").is_some()
                || payload.get("candidates").is_some() =>
            {
                return Err(());
            }
            // Native routes forward future provider events without treating
            // them as lifecycle evidence. Converters validate their own fields.
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gemini_native_continuation_is_incomplete_and_requires_its_opaque_token() {
        let response = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Partial answer"}]},
            "finishReason":"CONTINUATION","continuationToken":"c3ludGhldGlj"}]});
        assert_eq!(
            ResponseCompletion::for_protocol(WireApi::Gemini, &response),
            ResponseCompletion::Incomplete
        );
        let mut completion = GeminiStreamCompletion::default();
        completion.observe(&response).unwrap();
        completion
            .observe(&json!({"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}))
            .unwrap();
        assert_eq!(completion.outcome(), ResponseCompletion::Incomplete);
        assert!(completion.observe(&response).is_err());
        for token in [Value::Null, json!(""), json!(false), json!(1), json!({})] {
            let mut invalid = response.clone();
            invalid["candidates"][0]["continuationToken"] = token;
            assert_eq!(
                ResponseCompletion::for_protocol(WireApi::Gemini, &invalid),
                ResponseCompletion::Invalid
            );
        }
    }

    #[test]
    fn chat_usage_chunk_requires_finished_choices_and_is_last_before_done() {
        let start =
            json!({"choices":[{"index":0,"delta":{"content":"answer"},"finish_reason":null}]});
        let end = json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
        let usage = json!({"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}});
        let mut completion = ChatStreamCompletion::default();
        assert!(completion.observe(&usage).is_err());
        completion.observe(&start).unwrap();
        assert_eq!(completion.outcome(), ResponseCompletion::Invalid);
        assert!(completion.observe(&usage).is_err());
        completion.observe(&end).unwrap();
        assert_eq!(completion.outcome(), ResponseCompletion::Completed);
        completion.observe(&usage).unwrap();
        assert!(completion.observe(&usage).is_err());
        assert!(completion.observe(&start).is_err());
        assert!(completion.observe(&end).is_err());
    }

    #[test]
    fn gemini_usage_can_follow_finish_but_finished_candidates_cannot_reopen() {
        let end = json!({"candidates":[{"index":0,"content":{"parts":[{"text":"answer"}]},"finishReason":"STOP"}]});
        let mut completion = GeminiStreamCompletion::default();
        completion.observe(&end).unwrap();
        completion
            .observe(&json!({"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}))
            .unwrap();
        assert_eq!(completion.outcome(), ResponseCompletion::Completed);
        assert!(completion.observe(&end).is_err());
        assert!(
            completion
                .observe(&json!({"candidates":[{"content":{"parts":[{"text":"late"}]}}]}))
                .is_err()
        );
    }

    #[test]
    fn messages_requires_closed_blocks_and_rejects_events_after_stop() {
        let mut completion = MessagesStreamCompletion::default();
        completion.observe(&json!({"type":"ping"})).unwrap();
        completion
            .observe(&json!({"type":"message_start","message":{"id":"synthetic-message"}}))
            .unwrap();
        completion.observe(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})).unwrap();
        assert!(
            completion
                .observe(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}))
                .is_err()
        );
        completion
            .observe(&json!({"type":"content_block_stop","index":0}))
            .unwrap();
        assert!(completion.observe(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"late"}})).is_err());
        completion
            .observe(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}))
            .unwrap();
        assert_eq!(
            completion.observe(&json!({"type":"message_stop"})).unwrap(),
            Some(ResponseCompletion::Completed)
        );
        for event in [
            json!({"type":"message_stop"}),
            json!({"type":"ping"}),
            json!({"type":"message_delta","delta":{},"usage":{"output_tokens":2}}),
        ] {
            assert!(completion.observe(&event).is_err());
        }
    }
}

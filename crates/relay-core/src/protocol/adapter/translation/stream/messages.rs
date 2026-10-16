use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn messages(
        &mut self,
        upstream_event: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        self.messages_completion
            .observe(upstream_event)
            .map_err(|()| invalid())?;
        match upstream_event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
        {
            "ping" => {}
            "message_start" => {}
            "content_block_start" => {
                let key = upstream_event
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?
                    .to_string();
                let native = upstream_event.get("content_block").ok_or_else(invalid)?;
                super::super::super::messages::validate_response_block(native)
                    .map_err(|_| invalid())?;
                if self.native_indices.contains_key(&key) {
                    return Err(invalid());
                }
                let block = match native.get("type").and_then(Value::as_str) {
                    Some("text") => Block::Text(
                        native
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    ),
                    Some("thinking") => Block::Reasoning(
                        native
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    ),
                    Some("tool_use") => Block::ToolCall {
                        id: required_text(native, "id")?.into(),
                        name: required_text(native, "name")?.into(),
                        arguments: native
                            .get("input")
                            .filter(|tool_input| {
                                tool_input
                                    .as_object()
                                    .is_some_and(|tool_input_fields| !tool_input_fields.is_empty())
                            })
                            .map(Value::to_string)
                            .unwrap_or_default(),
                    },
                    Some("redacted_thinking") => {
                        required_text(native, "data").map_err(|_| invalid())?;
                        let index = self.decoded_response.native_output.len();
                        self.native_indices.insert(key, index);
                        self.decoded_response.native_output.push(native.clone());
                        return Ok(false);
                    }
                    _ => return Err(invalid()),
                };
                let native_index = self.decoded_response.native_output.len();
                self.native_indices.insert(key.clone(), native_index);
                self.decoded_response.native_output.push(native.clone());
                self.insert(key, block)?;
            }
            "content_block_delta" => {
                let key = upstream_event
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?
                    .to_string();
                let index = *self.indices.get(&key).ok_or_else(invalid)?;
                let native_index = *self.native_indices.get(&key).ok_or_else(invalid)?;
                if self.native_closed.contains(&native_index) {
                    return Err(invalid());
                }
                let delta = upstream_event.get("delta").ok_or_else(invalid)?;
                super::super::super::messages::validate_response_delta(delta)?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta" | "thinking_delta" | "input_json_delta") => {}
                    Some("signature_delta") => {
                        let signature = delta
                            .get("signature")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?;
                        if !matches!(self.decoded_response.blocks[index], Block::Reasoning(_)) {
                            return Err(invalid());
                        }
                        let native = &mut self.decoded_response.native_output[native_index];
                        let previous = native
                            .get("signature")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        native["signature"] = format!("{previous}{signature}").into();
                        return Ok(false);
                    }
                    _ => return Err(invalid()),
                }
                match (
                    &mut self.decoded_response.blocks[index],
                    delta.get("type").and_then(Value::as_str),
                ) {
                    (Block::Text(text), Some("text_delta")) => text.push_str(
                        delta
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?,
                    ),
                    (Block::Reasoning(text), Some("thinking_delta")) => text.push_str(
                        delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?,
                    ),
                    (Block::ToolCall { arguments, .. }, Some("input_json_delta")) => arguments
                        .push_str(
                            delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .ok_or_else(invalid)?,
                        ),
                    _ => return Err(invalid()),
                }
                let (field, text) = match &self.decoded_response.blocks[index] {
                    Block::Text(text) => ("text", text),
                    Block::Reasoning(text) => ("thinking", text),
                    Block::ToolCall { .. } => return Ok(false),
                    _ => return Err(invalid()),
                };
                self.decoded_response.native_output[native_index][field] = text.clone().into();
            }
            "content_block_stop" => {
                let key = upstream_event
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?
                    .to_string();
                let native_index = *self.native_indices.get(&key).ok_or_else(invalid)?;
                if !self.native_closed.insert(native_index) {
                    return Err(invalid());
                }
                let Some(&index) = self.indices.get(&key) else {
                    return Ok(false);
                };
                self.closed.insert(index);
                if matches!(self.decoded_response.blocks[index], Block::Reasoning(_)) {
                    required_text(
                        &self.decoded_response.native_output[native_index],
                        "signature",
                    )
                    .map_err(|_| invalid())?;
                }
                if let Block::ToolCall { arguments, .. } = &mut self.decoded_response.blocks[index]
                {
                    if arguments.is_empty() {
                        *arguments = "{}".into();
                    }
                    let input = serde_json::from_str::<Value>(arguments).map_err(|_| invalid())?;
                    if !input.is_object() {
                        return Err(invalid());
                    }
                    self.decoded_response.native_output[native_index]["input"] = input;
                }
            }
            "message_delta" => {
                if let Some(reason) = upstream_event
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                {
                    if self.finish_reason.is_some() {
                        return Err(invalid());
                    }
                    self.finish_reason = Some(response::finish(WireApi::Messages, reason)?);
                    self.refusal_explanation = upstream_event
                        .pointer("/delta/stop_details/explanation")
                        .or_else(|| upstream_event.pointer("/stop_details/explanation"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            "message_stop" => {
                if self.native_closed.len() != self.decoded_response.native_output.len()
                    || self.finish_reason.is_none()
                {
                    return Err(invalid());
                }
                if self.finish_reason == Some(Finish::Refusal) {
                    self.decoded_response.blocks.push(Block::Refusal(
                        self.refusal_explanation.take().unwrap_or_default(),
                    ));
                }
                return Ok(true);
            }
            _ => return Err(invalid()),
        }
        Ok(false)
    }
}

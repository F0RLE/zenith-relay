use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn messages(
        &mut self,
        value: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        match value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
        {
            "ping" => {}
            "message_start" => {
                self.merge_usage(response::usage(
                    WireApi::Messages,
                    value.get("message").ok_or_else(invalid)?,
                ));
            }
            "content_block_start" => {
                let key = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?
                    .to_string();
                let block = value.get("content_block").ok_or_else(invalid)?;
                let block = match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        checked(block, &["type", "text"])?;
                        Block::Text(
                            block
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                        )
                    }
                    Some("thinking") => {
                        // Signed thinking is provider-owned state. The generic
                        // translator cannot carry it safely across protocols;
                        // the dedicated MessagesStreamBridge handles it.
                        checked(block, &["type", "thinking"])?;
                        Block::Reasoning(
                            block
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                        )
                    }
                    Some("tool_use") => {
                        checked(block, &["type", "id", "name", "input"])?;
                        Block::ToolCall {
                            id: required_text(block, "id")?.into(),
                            name: required_text(block, "name")?.into(),
                            arguments: block
                                .get("input")
                                .filter(|input| {
                                    input.as_object().is_some_and(|object| !object.is_empty())
                                })
                                .map(Value::to_string)
                                .unwrap_or_default(),
                        }
                    }
                    _ => return Err(invalid()),
                };
                self.insert(key, block)?;
            }
            "content_block_delta" => {
                let key = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?
                    .to_string();
                let index = *self.indices.get(&key).ok_or_else(invalid)?;
                if self.closed.contains(&index) {
                    return Err(invalid());
                }
                let delta = value.get("delta").ok_or_else(invalid)?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => checked(delta, &["type", "text"])?,
                    Some("thinking_delta") => checked(delta, &["type", "thinking"])?,
                    Some("input_json_delta") => checked(delta, &["type", "partial_json"])?,
                    _ => return Err(invalid()),
                }
                match (
                    &mut self.response.blocks[index],
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
            }
            "content_block_stop" => {
                let key = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?
                    .to_string();
                let index = *self.indices.get(&key).ok_or_else(invalid)?;
                if !self.closed.insert(index) {
                    return Err(invalid());
                }
                if let Block::ToolCall { arguments, .. } = &mut self.response.blocks[index] {
                    if arguments.is_empty() {
                        *arguments = "{}".into();
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = value.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.finish_reason = Some(response::finish(WireApi::Messages, reason)?);
                }
            }
            "message_stop" => {
                if self.closed.len() != self.response.blocks.len() || self.finish_reason.is_none() {
                    return Err(invalid());
                }
                return Ok(true);
            }
            _ => return Err(invalid()),
        }
        Ok(false)
    }
}

use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn gemini(
        &mut self,
        upstream_event: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        if super::super::super::gemini::prompt_blocked(upstream_event).map_err(|()| invalid())? {
            if !self.decoded_response.blocks.is_empty() || self.finish_reason.is_some() {
                return Err(invalid());
            }
            self.finish_reason = Some(Finish::Filter);
            return Ok(true);
        }
        let Some(candidates) = upstream_event.get("candidates").and_then(Value::as_array) else {
            return if upstream_event.get("usageMetadata").is_some() {
                Ok(false)
            } else {
                Err(invalid())
            };
        };
        if candidates.len() != 1 {
            return Err(invalid());
        }
        let candidate = &candidates[0];
        if candidate
            .get("index")
            .and_then(Value::as_u64)
            .is_some_and(|index| index != 0)
        {
            return Err(invalid());
        }
        if let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        {
            for part in parts {
                checked(part, &["text", "thought", "functionCall"])?;
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    let reasoning = part.get("thought").and_then(Value::as_bool) == Some(true);
                    match self.decoded_response.blocks.last_mut() {
                        Some(Block::Text(block_text)) if !reasoning => block_text.push_str(text),
                        Some(Block::Reasoning(block_text)) if reasoning => {
                            block_text.push_str(text)
                        }
                        _ => self.decoded_response.blocks.push(if reasoning {
                            Block::Reasoning(text.into())
                        } else {
                            Block::Text(text.into())
                        }),
                    }
                } else if let Some(call) = part.get("functionCall") {
                    checked(call, &["id", "name", "args"])?;
                    let call_id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            format!(
                                "{}_call_{}",
                                self.decoded_response.id,
                                self.decoded_response.blocks.len()
                            )
                        });
                    let arguments = call
                        .get("args")
                        .filter(|arguments_value| arguments_value.is_object())
                        .ok_or_else(invalid)?
                        .to_string();
                    self.decoded_response.blocks.push(Block::ToolCall {
                        id: call_id,
                        name: required_text(call, "name")?.into(),
                        arguments,
                    });
                } else {
                    return Err(invalid());
                }
            }
        }
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            let mut finish = response::finish(WireApi::Gemini, reason)?;
            if finish == Finish::Stop
                && self
                    .decoded_response
                    .blocks
                    .iter()
                    .any(|block| matches!(block, Block::ToolCall { .. }))
            {
                finish = Finish::Tools;
            }
            self.finish_reason = Some(finish);
            return Ok(true);
        }
        Ok(false)
    }
}

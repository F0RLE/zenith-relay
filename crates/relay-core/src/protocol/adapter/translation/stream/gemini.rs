use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn gemini(
        &mut self,
        upstream_event: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        self.gemini_completion
            .observe(upstream_event)
            .map_err(|()| invalid())?;
        if self.finish_reason.is_some() {
            return if upstream_event
                .get("candidates")
                .is_none_or(|candidates| candidates.as_array().is_some_and(Vec::is_empty))
                && upstream_event.get("promptFeedback").is_none()
                && upstream_event
                    .get("usageMetadata")
                    .is_some_and(Value::is_object)
            {
                Ok(false)
            } else {
                Err(invalid())
            };
        }
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
        if candidates.is_empty()
            && upstream_event
                .get("usageMetadata")
                .is_some_and(Value::is_object)
        {
            return Ok(false);
        }
        if candidates.len() != 1 {
            return Err(invalid());
        }
        let candidate = &candidates[0];
        if candidate
            .get("index")
            .is_some_and(|index| index.as_u64() != Some(0))
        {
            return Err(invalid());
        }
        if let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        {
            for part in parts {
                super::super::super::gemini::validate_response_part(part).map_err(|_| invalid())?;
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    let reasoning = part.get("thought").and_then(Value::as_bool) == Some(true);
                    let index = self.decoded_response.blocks.len().saturating_sub(1);
                    let native_index = self.native_indices.get(&index.to_string()).copied();
                    let append = part.get("thoughtSignature").is_none()
                        && native_index.is_some_and(|native_index| {
                            self.decoded_response.native_output[native_index]
                                .get("thoughtSignature")
                                .is_none()
                        });
                    match self.decoded_response.blocks.last_mut() {
                        Some(Block::Text(block_text)) if !reasoning && append => {
                            block_text.push_str(text);
                            let Some(native_index) = native_index else {
                                return Err(invalid());
                            };
                            let native = self
                                .decoded_response
                                .native_output
                                .get_mut(native_index)
                                .ok_or_else(invalid)?;
                            let previous = native["text"].as_str().ok_or_else(invalid)?;
                            native["text"] = format!("{previous}{text}").into();
                        }
                        Some(Block::Reasoning(block_text)) if reasoning && append => {
                            block_text.push_str(text);
                            let Some(native_index) = native_index else {
                                return Err(invalid());
                            };
                            let native = self
                                .decoded_response
                                .native_output
                                .get_mut(native_index)
                                .ok_or_else(invalid)?;
                            let previous = native["text"].as_str().ok_or_else(invalid)?;
                            native["text"] = format!("{previous}{text}").into();
                        }
                        _ => {
                            if !self.decoded_response.blocks.is_empty() {
                                self.closed.insert(index);
                            }
                            self.native_indices.insert(
                                self.decoded_response.blocks.len().to_string(),
                                self.decoded_response.native_output.len(),
                            );
                            self.decoded_response.native_output.push(part.clone());
                            self.decoded_response.blocks.push(if reasoning {
                                Block::Reasoning(text.into())
                            } else {
                                Block::Text(text.into())
                            });
                        }
                    }
                } else if part.get("functionCall").is_some() {
                    self.gemini_call(part)?;
                } else if part.get("thoughtSignature").is_some() {
                    self.decoded_response.native_output.push(part.clone());
                } else {
                    return Err(invalid());
                }
            }
        }
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            if self.gemini_calls.has_open_calls() {
                return Err(invalid());
            }
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

    fn gemini_call(&mut self, part: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let (call_index, is_new) = self.gemini_calls.push(part)?;
        let state = self.gemini_calls.get(call_index);
        let name = state.name.clone();
        let native_part = state.native_part();
        let arguments = state.arguments.to_string();
        let key = format!("gemini-call:{call_index}");
        let index = if is_new {
            let index = self.decoded_response.blocks.len();
            let id = state
                .provider_id
                .clone()
                .unwrap_or_else(|| format!("{}_call_{index}", self.decoded_response.id));
            let native_index = self.decoded_response.native_output.len();
            self.native_indices.insert(index.to_string(), native_index);
            self.decoded_response
                .native_output
                .push(native_part.clone());
            self.insert(
                key,
                Block::ToolCall {
                    id,
                    name,
                    arguments: arguments.clone(),
                },
            )?
        } else {
            *self.indices.get(&key).ok_or_else(invalid)?
        };
        let native_index = *self
            .native_indices
            .get(&index.to_string())
            .ok_or_else(invalid)?;
        self.decoded_response.native_output[native_index] = native_part;
        let Block::ToolCall {
            arguments: text, ..
        } = &mut self.decoded_response.blocks[index]
        else {
            return Err(invalid());
        };
        *text = arguments;
        Ok(())
    }
}

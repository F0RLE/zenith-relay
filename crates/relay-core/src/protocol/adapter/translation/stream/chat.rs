use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn chat(
        &mut self,
        upstream_event: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        self.chat_completion
            .observe(upstream_event)
            .map_err(|_| invalid())?;
        let choices = upstream_event
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        if choices.is_empty() {
            return Ok(false);
        }
        if choices.len() != 1 || choices[0].get("index").and_then(Value::as_u64) != Some(0) {
            return Err(invalid());
        }
        let choice = &choices[0];
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            let finish = response::finish(WireApi::ChatCompletions, reason)?;
            self.finish_reason = Some(if self.saw_refusal && finish == Finish::Stop {
                Finish::Refusal
            } else {
                finish
            });
        }
        let delta = choice.get("delta").ok_or_else(invalid)?;
        checked(
            delta,
            &[
                "role",
                "content",
                "tool_calls",
                "reasoning_content",
                "refusal",
                "annotations",
            ],
        )?;
        if let Some(annotations) = delta.get("annotations").filter(|value| !value.is_null()) {
            for annotation in annotations.as_array().ok_or_else(invalid)? {
                if !self.chat_annotations.contains(annotation) {
                    self.chat_annotations.push(annotation.clone());
                }
            }
        }
        if delta
            .get("role")
            .is_some_and(|role| !role.is_null() && role.as_str() != Some("assistant"))
        {
            return Err(invalid());
        }
        if let Some(refusal) = delta
            .get("refusal")
            .filter(|refusal_value| !refusal_value.is_null())
        {
            let refusal = refusal.as_str().ok_or_else(invalid)?;
            self.saw_refusal = true;
            let index = match self.indices.get("refusal") {
                Some(index) => *index,
                None => self.insert("refusal".into(), Block::Refusal(String::new()))?,
            };
            let Block::Refusal(text) = &mut self.decoded_response.blocks[index] else {
                return Err(invalid());
            };
            text.push_str(refusal);
        }
        for (field, key, reasoning) in [
            ("reasoning_content", "reasoning", true),
            ("content", "text", false),
        ] {
            if let Some(text_value) = delta
                .get(field)
                .filter(|field_value| !field_value.is_null())
            {
                let text = text_value.as_str().ok_or_else(invalid)?;
                let index = match self.indices.get(key) {
                    Some(index) => *index,
                    None => self.insert(
                        key.into(),
                        if reasoning {
                            Block::Reasoning(String::new())
                        } else {
                            Block::Text(String::new())
                        },
                    )?,
                };
                match &mut self.decoded_response.blocks[index] {
                    Block::Text(block_text) | Block::Reasoning(block_text) => {
                        block_text.push_str(text)
                    }
                    _ => return Err(invalid()),
                }
            }
        }
        if let Some(calls) = delta
            .get("tool_calls")
            .filter(|tool_calls_value| !tool_calls_value.is_null())
        {
            for call in calls.as_array().ok_or_else(invalid)? {
                checked(call, &["index", "id", "type", "function"])?;
                if call
                    .get("type")
                    .is_some_and(|kind| !kind.is_null() && kind.as_str() != Some("function"))
                {
                    return Err(invalid());
                }
                let key = format!(
                    "tool:{}",
                    call.get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(invalid)?
                );
                let index = match self.indices.get(&key) {
                    Some(index) => *index,
                    None => self.insert(
                        key,
                        Block::ToolCall {
                            id: String::new(),
                            name: String::new(),
                            arguments: String::new(),
                        },
                    )?,
                };
                let Block::ToolCall {
                    id,
                    name,
                    arguments,
                } = &mut self.decoded_response.blocks[index]
                else {
                    return Err(invalid());
                };
                if let Some(fragment) = call.get("id").filter(|value| !value.is_null()) {
                    id.push_str(fragment.as_str().ok_or_else(invalid)?);
                }
                if let Some(function) = call.get("function").filter(|value| !value.is_null()) {
                    checked(function, &["name", "arguments"])?;
                    if let Some(fragment) = function.get("name").filter(|value| !value.is_null()) {
                        name.push_str(fragment.as_str().ok_or_else(invalid)?);
                    }
                    if let Some(fragment) =
                        function.get("arguments").filter(|value| !value.is_null())
                    {
                        arguments.push_str(fragment.as_str().ok_or_else(invalid)?);
                    }
                }
            }
        }
        if self.saw_refusal && self.finish_reason == Some(Finish::Stop) {
            self.finish_reason = Some(Finish::Refusal);
        }
        Ok(false)
    }
}

use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn chat(
        &mut self,
        upstream_event: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
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
            self.finish_reason = Some(if self.saw_refusal {
                Finish::Filter
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
            ],
        )?;
        if let Some(refusal) = delta
            .get("refusal")
            .filter(|refusal_value| !refusal_value.is_null())
        {
            let refusal = refusal.as_str().ok_or_else(invalid)?;
            if !refusal.is_empty() {
                self.saw_refusal = true;
                self.finish_reason = Some(Finish::Filter);
                let index = match self.indices.get("text") {
                    Some(index) => *index,
                    None => self.insert("text".into(), Block::Text(String::new()))?,
                };
                let Block::Text(text) = &mut self.decoded_response.blocks[index] else {
                    return Err(invalid());
                };
                text.push_str(refusal);
            }
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
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind != "function")
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
                if let Some(fragment) = call.get("id").and_then(Value::as_str) {
                    id.push_str(fragment);
                }
                if let Some(function) = call.get("function") {
                    checked(function, &["name", "arguments"])?;
                    if let Some(fragment) = function.get("name").and_then(Value::as_str) {
                        name.push_str(fragment);
                    }
                    if let Some(fragment) = function.get("arguments").and_then(Value::as_str) {
                        arguments.push_str(fragment);
                    }
                }
            }
        }
        Ok(false)
    }
}

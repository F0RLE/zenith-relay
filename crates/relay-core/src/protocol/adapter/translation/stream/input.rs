use super::*;

impl TranslationStream {
    pub(super) fn chat(&mut self, value: &Value) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        let choices = value
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
            self.finish_reason = Some(response::finish(WireApi::ChatCompletions, reason)?);
        }
        let delta = choice.get("delta").ok_or_else(invalid)?;
        checked(
            delta,
            &["role", "content", "tool_calls", "reasoning_content"],
        )?;
        for (field, key, reasoning) in [
            ("content", "text", false),
            ("reasoning_content", "reasoning", true),
        ] {
            if let Some(text) = delta.get(field).filter(|value| !value.is_null()) {
                let text = text.as_str().ok_or_else(invalid)?;
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
                match &mut self.response.blocks[index] {
                    Block::Text(value) | Block::Reasoning(value) => value.push_str(text),
                    _ => return Err(invalid()),
                }
            }
        }
        if let Some(calls) = delta.get("tool_calls").filter(|value| !value.is_null()) {
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
                } = &mut self.response.blocks[index]
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

    pub(super) fn responses(&mut self, value: &Value) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        match value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
        {
            "response.created" | "response.in_progress" => {}
            "response.output_item.added" => {
                let index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                let item = value.get("item").ok_or_else(invalid)?;
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => {
                        self.insert(
                            format!("output:{index}"),
                            Block::ToolCall {
                                id: required_text(item, "call_id")?.into(),
                                name: required_text(item, "name")?.into(),
                                arguments: item
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .into(),
                            },
                        )?;
                    }
                    Some("message") => {}
                    Some("reasoning") => {
                        if item
                            .get("encrypted_content")
                            .is_some_and(|value| !value.is_null())
                        {
                            return Err(invalid());
                        }
                    }
                    _ => return Err(invalid()),
                }
            }
            "response.content_part.added" | "response.reasoning_summary_part.added" => {
                let reasoning = value.get("type").and_then(Value::as_str)
                    == Some("response.reasoning_summary_part.added");
                let index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                let part_index = value
                    .get(if reasoning {
                        "summary_index"
                    } else {
                        "content_index"
                    })
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                let part = value.get("part").ok_or_else(invalid)?;
                if part.get("type").and_then(Value::as_str)
                    != Some(if reasoning {
                        "summary_text"
                    } else {
                        "output_text"
                    })
                {
                    return Err(invalid());
                }
                let text = part
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.insert(
                    format!("part:{index}:{part_index}"),
                    if reasoning {
                        Block::Reasoning(text)
                    } else {
                        Block::Text(text)
                    },
                )?;
            }
            "response.output_text.delta" | "response.reasoning_summary_text.delta" => {
                let reasoning = value.get("type").and_then(Value::as_str)
                    == Some("response.reasoning_summary_text.delta");
                let index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                let part = value
                    .get(if reasoning {
                        "summary_index"
                    } else {
                        "content_index"
                    })
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                let key = format!("part:{index}:{part}");
                let index = *self.indices.get(&key).ok_or_else(invalid)?;
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                match &mut self.response.blocks[index] {
                    Block::Text(text) | Block::Reasoning(text) => text.push_str(delta),
                    _ => return Err(invalid()),
                }
            }
            "response.function_call_arguments.delta" => {
                let index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                let index = *self
                    .indices
                    .get(&format!("output:{index}"))
                    .ok_or_else(invalid)?;
                let Block::ToolCall { arguments, .. } = &mut self.response.blocks[index] else {
                    return Err(invalid());
                };
                arguments.push_str(
                    value
                        .get("delta")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?,
                );
            }
            "response.output_text.done"
            | "response.content_part.done"
            | "response.output_item.done"
            | "response.function_call_arguments.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => {}
            "response.completed" | "response.incomplete" => {
                let response = response::decode(
                    WireApi::Responses,
                    value.get("response").ok_or_else(invalid)?,
                    &self.response.id,
                )?;
                self.finish_reason = Some(response.finish);
                self.merge_usage(response.usage.clone());
                self.response.blocks = response.blocks;
                return Ok(true);
            }
            "response.failed" => {
                self.upstream_error = value.get("response").cloned();
                return Err(invalid());
            }
            _ => return Err(invalid()),
        }
        Ok(false)
    }

    pub(super) fn messages(&mut self, value: &Value) -> AdapterResult<bool> {
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
                    Some("text") => Block::Text(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    ),
                    Some("tool_use") => Block::ToolCall {
                        id: required_text(block, "id")?.into(),
                        name: required_text(block, "name")?.into(),
                        arguments: block
                            .get("input")
                            .filter(|input| {
                                input.as_object().is_some_and(|object| !object.is_empty())
                            })
                            .map(Value::to_string)
                            .unwrap_or_default(),
                    },
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

    pub(super) fn gemini(&mut self, value: &Value) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        let Some(candidates) = value.get("candidates").and_then(Value::as_array) else {
            return if value.get("usageMetadata").is_some() {
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
                    match self.response.blocks.last_mut() {
                        Some(Block::Text(value)) if !reasoning => value.push_str(text),
                        Some(Block::Reasoning(value)) if reasoning => value.push_str(text),
                        _ => self.response.blocks.push(if reasoning {
                            Block::Reasoning(text.into())
                        } else {
                            Block::Text(text.into())
                        }),
                    }
                } else if let Some(call) = part.get("functionCall") {
                    checked(call, &["id", "name", "args"])?;
                    let id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            format!("{}_call_{}", self.response.id, self.response.blocks.len())
                        });
                    let arguments = call
                        .get("args")
                        .filter(|value| value.is_object())
                        .ok_or_else(invalid)?
                        .to_string();
                    self.response.blocks.push(Block::ToolCall {
                        id,
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
                    .response
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

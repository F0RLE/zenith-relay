use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn responses(
        &mut self,
        value: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        match value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
        {
            "response.created" | "response.in_progress" => {}
            "response.output_item.added" => self.add_output_item(value)?,
            "response.content_part.added" | "response.reasoning_summary_part.added" => {
                self.add_content_part(value)?
            }
            "response.output_text.delta" | "response.reasoning_summary_text.delta" => {
                self.append_text_delta(value)?
            }
            "response.function_call_arguments.delta" => self.append_arguments_delta(value)?,
            "response.output_text.done"
            | "response.content_part.done"
            | "response.output_item.done"
            | "response.function_call_arguments.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => {}
            "response.completed" | "response.incomplete" => {
                return self.finish_translated_response(value)
            }
            "response.failed" => {
                self.upstream_error = value.get("response").cloned();
                return Err(invalid());
            }
            _ => return Err(invalid()),
        }
        Ok(false)
    }

    fn add_output_item(&mut self, value: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
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
        Ok(())
    }

    fn add_content_part(&mut self, value: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
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
        Ok(())
    }

    fn append_text_delta(&mut self, value: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
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
        Ok(())
    }

    fn append_arguments_delta(&mut self, value: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
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
        Ok(())
    }

    fn finish_translated_response(&mut self, value: &Value) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        let response = response::decode(
            WireApi::Responses,
            value.get("response").ok_or_else(invalid)?,
            &self.response.id,
        )?;
        self.finish_reason = Some(response.finish);
        self.merge_usage(response.usage.clone());
        self.response.blocks = response.blocks;
        Ok(true)
    }
}

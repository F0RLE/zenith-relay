use super::*;

impl TranslationStream {
    pub(in crate::protocol::adapter::translation::stream) fn responses(
        &mut self,
        event_payload: &Value,
    ) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        match event_payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
        {
            "response.created" | "response.in_progress" => {}
            "response.output_item.added" => self.add_output_item(event_payload)?,
            "response.content_part.added" | "response.reasoning_summary_part.added" => {
                self.add_content_part(event_payload)?
            }
            "response.output_text.delta" | "response.reasoning_summary_text.delta" => {
                self.append_text_delta(event_payload)?
            }
            "response.function_call_arguments.delta" => {
                self.append_arguments_delta(event_payload)?
            }
            "response.output_text.done"
            | "response.content_part.done"
            | "response.output_item.done"
            | "response.function_call_arguments.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => {}
            "response.completed" | "response.incomplete" => {
                return self.finish_translated_response(event_payload)
            }
            "response.failed" => {
                self.upstream_error = event_payload.get("response").cloned();
                return Err(invalid());
            }
            _ => return Err(invalid()),
        }
        Ok(false)
    }

    fn add_output_item(&mut self, event_payload: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let index = event_payload
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let response_item = event_payload.get("item").ok_or_else(invalid)?;
        match response_item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                self.insert(
                    format!("output:{index}"),
                    Block::ToolCall {
                        id: required_text(response_item, "call_id")?.into(),
                        name: required_text(response_item, "name")?.into(),
                        arguments: response_item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    },
                )?;
            }
            Some("message") => {}
            Some("reasoning") => {
                if response_item
                    .get("encrypted_content")
                    .is_some_and(|encrypted_content_value| !encrypted_content_value.is_null())
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }

    fn add_content_part(&mut self, event_payload: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let reasoning = event_payload.get("type").and_then(Value::as_str)
            == Some("response.reasoning_summary_part.added");
        let index = event_payload
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let part_index = event_payload
            .get(if reasoning {
                "summary_index"
            } else {
                "content_index"
            })
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let content_part = event_payload.get("part").ok_or_else(invalid)?;
        if content_part.get("type").and_then(Value::as_str)
            != Some(if reasoning {
                "summary_text"
            } else {
                "output_text"
            })
        {
            return Err(invalid());
        }
        let text_value = content_part
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        self.insert(
            format!("part:{index}:{part_index}"),
            if reasoning {
                Block::Reasoning(text_value)
            } else {
                Block::Text(text_value)
            },
        )?;
        Ok(())
    }

    fn append_text_delta(&mut self, event_payload: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let reasoning = event_payload.get("type").and_then(Value::as_str)
            == Some("response.reasoning_summary_text.delta");
        let index = event_payload
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let content_part_index = event_payload
            .get(if reasoning {
                "summary_index"
            } else {
                "content_index"
            })
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let key = format!("part:{index}:{content_part_index}");
        let block_index = *self.indices.get(&key).ok_or_else(invalid)?;
        let delta = event_payload
            .get("delta")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        match &mut self.decoded_response.blocks[block_index] {
            Block::Text(text) | Block::Reasoning(text) => text.push_str(delta),
            _ => return Err(invalid()),
        }
        Ok(())
    }

    fn append_arguments_delta(&mut self, event_payload: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let output_index = event_payload
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let block_index = *self
            .indices
            .get(&format!("output:{output_index}"))
            .ok_or_else(invalid)?;
        let Block::ToolCall { arguments, .. } = &mut self.decoded_response.blocks[block_index]
        else {
            return Err(invalid());
        };
        arguments.push_str(
            event_payload
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?,
        );
        Ok(())
    }

    fn finish_translated_response(&mut self, event_payload: &Value) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        let decoded_response = response::decode(
            WireApi::Responses,
            event_payload.get("response").ok_or_else(invalid)?,
            &self.decoded_response.id,
        )?;
        self.finish_reason = Some(decoded_response.finish);
        self.merge_usage(decoded_response.usage.clone());
        self.decoded_response.blocks = decoded_response.blocks;
        Ok(true)
    }
}

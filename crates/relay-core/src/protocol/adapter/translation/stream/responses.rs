use super::*;
mod snapshots;

// Only fixed protocol labels may enter diagnostics, never an upstream-supplied
// event type, item identity, tool arguments or response content.
pub(super) fn diagnostic_parameter(event: &Value) -> &'static str {
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_item.added") => "response.output_item.added",
        Some("response.output_item.done") => "response.output_item.done",
        Some("response.content_part.added" | "response.reasoning_summary_part.added") => {
            "response.content_part.added"
        }
        Some("response.content_part.done" | "response.reasoning_summary_part.done") => {
            "response.content_part.done"
        }
        Some(
            "response.output_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.refusal.delta",
        ) => "response.text.delta",
        Some(
            "response.output_text.done"
            | "response.reasoning_summary_text.done"
            | "response.refusal.done",
        ) => "response.text.done",
        Some(
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta",
        ) => "response.tool_input.delta",
        Some("response.function_call_arguments.done" | "response.custom_tool_call_input.done") => {
            "response.tool_input.done"
        }
        Some("response.output_text.annotation.added") => "response.output_text.annotation.added",
        Some("response.completed" | "response.done" | "response.incomplete") => "response.terminal",
        Some("response.failed") => "response.failed",
        _ => "stream.responses.event",
    }
}

#[derive(Debug)]
pub(super) struct OutputItemState {
    item: Value,
    closed: bool,
    input_done: bool,
    text_done: BTreeSet<u64>,
}

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
            "response.output_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.refusal.delta" => self.append_text_delta(event_payload)?,
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                self.append_arguments_delta(event_payload)?
            }
            "response.output_text.annotation.added" => self.add_annotation(event_payload)?,
            "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
            | "response.web_search_call.completed" => {}
            "response.output_text.done"
            | "response.refusal.done"
            | "response.reasoning_summary_text.done" => self.finish_text(event_payload)?,
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                self.finish_arguments(event_payload)?
            }
            "response.content_part.done" | "response.reasoning_summary_part.done" => {
                self.finish_content_part(event_payload)?
            }
            "response.output_item.done" => self.finish_output_item(event_payload)?,
            "response.completed" | "response.done" | "response.incomplete" => {
                return self.finish_translated_response(event_payload)
            }
            "response.failed" => {
                if !event_payload.get("response").is_some_and(Value::is_object) {
                    return Err(invalid());
                }
                // Preserve the provider's terminal envelope so `fail` can
                // forward the original error instead of replacing it with a
                // generic adapter message.
                self.upstream_error = Some(event_payload.clone());
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
        if !response_item.is_object()
            || self.response_items.contains_key(&index)
            || response_item.get("status").is_some_and(|status| {
                !status.is_null()
                    && !matches!(
                        status.as_str(),
                        Some("in_progress" | "completed" | "incomplete")
                    )
            })
        {
            return Err(invalid());
        }
        if matches!(
            response_item.get("type").and_then(Value::as_str),
            Some("function_call" | "custom_tool_call")
        ) {
            response::validate_responses_call_execution(response_item)?;
        }
        match response_item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                self.insert(
                    format!("output:{index}"),
                    Block::ToolCall {
                        id: required_text(response_item, "call_id")
                            .map_err(|_| invalid())?
                            .into(),
                        name: required_text(response_item, "name")
                            .map_err(|_| invalid())?
                            .into(),
                        arguments: response_item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?
                            .into(),
                    },
                )?;
            }
            Some("custom_tool_call") => {
                response_item
                    .get("input")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                self.insert(
                    format!("output:{index}"),
                    Block::ToolCall {
                        id: required_text(response_item, "call_id")
                            .map_err(|_| invalid())?
                            .into(),
                        name: required_text(response_item, "name")
                            .map_err(|_| invalid())?
                            .into(),
                        arguments: String::new(),
                    },
                )?;
            }
            Some("message" | "web_search_call") => {}
            Some("reasoning") => {
                if response_item
                    .get("encrypted_content")
                    .is_some_and(|value| !value.is_null() && !value.is_string())
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        self.response_items.insert(
            index,
            OutputItemState {
                item: response_item.clone(),
                closed: false,
                input_done: false,
                text_done: BTreeSet::new(),
            },
        );
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
        self.open_response_item(
            event_payload,
            if reasoning { "reasoning" } else { "message" },
        )?;
        let part_index = event_payload
            .get(if reasoning {
                "summary_index"
            } else {
                "content_index"
            })
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let content_part = event_payload.get("part").ok_or_else(invalid)?;
        let kind = content_part
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if (reasoning && kind != "summary_text")
            || (!reasoning && !matches!(kind, "output_text" | "refusal"))
        {
            return Err(invalid());
        }
        let text_value = content_part
            .get(if kind == "refusal" { "refusal" } else { "text" })
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .to_owned();
        self.insert(
            format!("part:{index}:{part_index}"),
            if kind == "refusal" {
                Block::Refusal(text_value)
            } else if reasoning {
                Block::Reasoning(text_value)
            } else {
                Block::Text(text_value)
            },
        )?;
        if kind == "output_text" {
            self.retain_annotations(index, part_index, content_part)?;
        }
        Ok(())
    }

    fn append_text_delta(&mut self, event_payload: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let reasoning = event_payload.get("type").and_then(Value::as_str)
            == Some("response.reasoning_summary_text.delta");
        self.open_response_item(
            event_payload,
            if reasoning { "reasoning" } else { "message" },
        )?;
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
        if self.closed.contains(&block_index)
            || self.response_items[&index]
                .text_done
                .contains(&content_part_index)
        {
            return Err(invalid());
        }
        let delta = event_payload
            .get("delta")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        match (
            &mut self.decoded_response.blocks[block_index],
            event_payload.get("type").and_then(Value::as_str),
        ) {
            (Block::Text(text), Some("response.output_text.delta"))
            | (Block::Reasoning(text), Some("response.reasoning_summary_text.delta"))
            | (Block::Refusal(text), Some("response.refusal.delta")) => text.push_str(delta),
            _ => return Err(invalid()),
        }
        Ok(())
    }

    fn append_arguments_delta(&mut self, event_payload: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let custom = event_payload.get("type").and_then(Value::as_str)
            == Some("response.custom_tool_call_input.delta");
        self.open_response_item(
            event_payload,
            if custom {
                "custom_tool_call"
            } else {
                "function_call"
            },
        )?;
        let output_index = event_payload
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let state = self
            .response_items
            .get_mut(&output_index)
            .ok_or_else(invalid)?;
        if state.input_done {
            return Err(invalid());
        }
        let delta = event_payload
            .get("delta")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if custom {
            let input = state.item.get_mut("input").ok_or_else(invalid)?;
            let Value::String(input) = input else {
                return Err(invalid());
            };
            input.push_str(delta);
            return Ok(());
        }
        let block_index = *self
            .indices
            .get(&format!("output:{output_index}"))
            .ok_or_else(invalid)?;
        let Block::ToolCall { arguments, .. } = &mut self.decoded_response.blocks[block_index]
        else {
            return Err(invalid());
        };
        if self.closed.contains(&block_index) {
            return Err(invalid());
        }
        arguments.push_str(delta);
        Ok(())
    }

    fn open_response_item(&self, event: &Value, expected_type: &str) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let state = self.response_items.get(&index).ok_or_else(invalid)?;
        if state.closed
            || state.item.get("type").and_then(Value::as_str) != Some(expected_type)
            || event
                .get("item_id")
                .is_some_and(|id| Some(id) != state.item.get("id"))
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn finish_arguments(&mut self, event: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let custom = event.get("type").and_then(Value::as_str)
            == Some("response.custom_tool_call_input.done");
        self.open_response_item(
            event,
            if custom {
                "custom_tool_call"
            } else {
                "function_call"
            },
        )?;
        let index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let state = self.response_items.get_mut(&index).ok_or_else(invalid)?;
        if state.input_done {
            return Err(invalid());
        }
        let field = if custom { "input" } else { "arguments" };
        let final_input = event
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let block_index = *self
            .indices
            .get(&format!("output:{index}"))
            .ok_or_else(invalid)?;
        let Block::ToolCall { arguments, .. } = &mut self.decoded_response.blocks[block_index]
        else {
            return Err(invalid());
        };
        let previous = if custom {
            state.item["input"].as_str().ok_or_else(invalid)?
        } else {
            arguments.as_str()
        };
        if !final_input.starts_with(previous) {
            return Err(invalid());
        }
        *arguments = if custom {
            json!({"input": final_input}).to_string()
        } else {
            final_input.into()
        };
        state.item[field] = final_input.into();
        state.input_done = true;
        Ok(())
    }

    fn finish_text(&mut self, event: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let kind = event.get("type").and_then(Value::as_str);
        let reasoning = kind == Some("response.reasoning_summary_text.done");
        let refusal = kind == Some("response.refusal.done");
        let output_index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let part_index = event
            .get(if reasoning {
                "summary_index"
            } else {
                "content_index"
            })
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        if self
            .response_items
            .get(&output_index)
            .is_none_or(|state| state.text_done.contains(&part_index))
        {
            return Err(invalid());
        }
        let mut part_event = event.clone();
        part_event["type"] = if reasoning {
            "response.reasoning_summary_part.done"
        } else {
            "response.content_part.done"
        }
        .into();
        part_event["part"] = if refusal {
            json!({"type":"refusal","refusal":event["refusal"]})
        } else {
            json!({"type":if reasoning { "summary_text" } else { "output_text" },"text":event["text"]})
        };
        self.update_content_part(&part_event, false)?;
        self.response_items
            .get_mut(&output_index)
            .ok_or_else(invalid)?
            .text_done
            .insert(part_index);
        Ok(())
    }

    fn finish_content_part(&mut self, event: &Value) -> AdapterResult<()> {
        self.update_content_part(event, true)
    }

    fn update_content_part(&mut self, event: &Value, close: bool) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let reasoning = event.get("type").and_then(Value::as_str)
            == Some("response.reasoning_summary_part.done");
        self.open_response_item(event, if reasoning { "reasoning" } else { "message" })?;
        let output_index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let part_index = event
            .get(if reasoning {
                "summary_index"
            } else {
                "content_index"
            })
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let key = format!("part:{output_index}:{part_index}");
        let block_index = *self.indices.get(&key).ok_or_else(invalid)?;
        let part = event.get("part").ok_or_else(invalid)?;
        let refusal = matches!(self.decoded_response.blocks[block_index], Block::Refusal(_));
        let expected_type = if reasoning {
            "summary_text"
        } else if refusal {
            "refusal"
        } else {
            "output_text"
        };
        if part.get("type").and_then(Value::as_str) != Some(expected_type) {
            return Err(invalid());
        }
        let final_text = part
            .get(if refusal { "refusal" } else { "text" })
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let text = match &mut self.decoded_response.blocks[block_index] {
            Block::Reasoning(text) if reasoning => text,
            Block::Refusal(text) if refusal => text,
            Block::Text(text) if !reasoning && !refusal => text,
            _ => return Err(invalid()),
        };
        if !final_text.starts_with(text.as_str())
            || (self.response_items[&output_index]
                .text_done
                .contains(&part_index)
                && final_text != text.as_str())
            || self.closed.contains(&block_index)
        {
            return Err(invalid());
        }
        if close {
            self.closed.insert(block_index);
        }
        text.push_str(&final_text[text.len()..]);
        if expected_type == "output_text" {
            self.retain_annotations(output_index, part_index, part)?;
        }
        Ok(())
    }

    fn finish_output_item(&mut self, event: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let output_index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        if self
            .response_items
            .get(&output_index)
            .is_none_or(|state| state.closed)
        {
            return Err(invalid());
        }
        let mut item = event.get("item").ok_or_else(invalid)?.clone();
        if let Some(parts) = item.get("content").and_then(Value::as_array) {
            for (index, part) in parts.iter().enumerate() {
                if part.get("type").and_then(Value::as_str) == Some("output_text") {
                    self.retain_annotations(output_index, index as u64, part)?;
                }
            }
        }
        self.merge_item_annotations(output_index, &mut item)?;
        let item_type = item.get("type").and_then(Value::as_str);
        self.validate_final_item(output_index, &item)?;
        let state = self
            .response_items
            .get_mut(&output_index)
            .ok_or_else(invalid)?;
        state.item = item.clone();
        state.closed = true;
        if item_type == Some("reasoning") {
            if item
                .get("encrypted_content")
                .is_some_and(|value| !value.is_null() && !value.is_string())
            {
                return Err(invalid());
            }
            return Ok(());
        }
        if item_type == Some("message") || item_type == Some("web_search_call") {
            return Ok(());
        }
        let block_index = *self
            .indices
            .get(&format!("output:{output_index}"))
            .ok_or_else(invalid)?;
        let Block::ToolCall {
            id,
            name,
            arguments,
        } = &mut self.decoded_response.blocks[block_index]
        else {
            return Err(invalid());
        };
        let (final_arguments, call_id, final_name) = match item_type {
            Some("function_call") => (
                item.get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?
                    .to_owned(),
                item.get("call_id").and_then(Value::as_str),
                item.get("name").and_then(Value::as_str),
            ),
            Some("custom_tool_call") => {
                let input = item
                    .get("input")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                (
                    json!({"input":input}).to_string(),
                    item.get("call_id").and_then(Value::as_str),
                    item.get("name").and_then(Value::as_str),
                )
            }
            _ => return Err(invalid()),
        };
        if call_id != Some(id.as_str())
            || final_name != Some(name.as_str())
            || !self.closed.insert(block_index)
        {
            return Err(invalid());
        }
        *arguments = final_arguments;
        response::validate_calls(
            &[self.decoded_response.blocks[block_index].clone()],
            if item.get("status").and_then(Value::as_str) == Some("incomplete") {
                Finish::Length
            } else {
                Finish::Tools
            },
        )
        .map_err(|_| invalid())
    }

    fn validate_final_item(&self, index: u64, item: &Value) -> AdapterResult<()> {
        let invalid =
            || AdapterError::upstream_stream_invalid().with_parameter("response.output_item");
        let state = self.response_items.get(&index).ok_or_else(|| {
            AdapterError::upstream_stream_invalid().with_parameter("response.output_item.missing")
        })?;
        if matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call" | "custom_tool_call")
        ) {
            response::validate_responses_call_execution(item)?;
        }
        for field in ["type", "id", "call_id", "name", "namespace"] {
            if state
                .item
                .get(field)
                .is_some_and(|value| item.get(field) != Some(value))
            {
                return Err(invalid().with_parameter("response.output_item.identity"));
            }
        }
        if state.closed {
            return snapshots::validate_unchanged(&state.item, item);
        }
        match item.get("type").and_then(Value::as_str) {
            Some("function_call" | "custom_tool_call") => {
                let custom = item["type"] == "custom_tool_call";
                let field = if custom { "input" } else { "arguments" };
                let index = *self
                    .indices
                    .get(&format!("output:{index}"))
                    .ok_or_else(invalid)?;
                let Block::ToolCall { arguments, .. } = &self.decoded_response.blocks[index] else {
                    return Err(invalid());
                };
                let previous = if custom {
                    state
                        .item
                        .get(field)
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?
                } else {
                    arguments.as_str()
                };
                let final_input = item
                    .get(field)
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                if !final_input.starts_with(previous)
                    || (state.input_done && final_input != previous)
                {
                    return Err(invalid().with_parameter("response.tool_input.changed"));
                }
            }
            Some("message" | "reasoning") => {
                let reasoning = item["type"] == "reasoning";
                for (key, &block_index) in &self.indices {
                    let prefix = format!("part:{index}:");
                    let Some(part_index) = key.strip_prefix(&prefix) else {
                        continue;
                    };
                    let part_index = part_index.parse::<usize>().map_err(|_| invalid())?;
                    let part = item
                        .get(if reasoning { "summary" } else { "content" })
                        .and_then(Value::as_array)
                        .and_then(|parts| parts.get(part_index))
                        .ok_or_else(invalid)?;
                    let (kind, field, text) = match &self.decoded_response.blocks[block_index] {
                        Block::Text(text) => ("output_text", "text", text),
                        Block::Reasoning(text) => ("summary_text", "text", text),
                        Block::Refusal(text) => ("refusal", "refusal", text),
                        _ => return Err(invalid()),
                    };
                    let final_text = part
                        .get(field)
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?;
                    if part.get("type").and_then(Value::as_str) != Some(kind)
                        || !final_text.starts_with(text)
                        || ((self.closed.contains(&block_index)
                            || state.text_done.contains(&(part_index as u64)))
                            && final_text != text)
                    {
                        return Err(invalid().with_parameter("response.text.changed"));
                    }
                }
            }
            Some("web_search_call") => {}
            _ => return Err(invalid()),
        }
        Ok(())
    }

    fn finish_translated_response(&mut self, event_payload: &Value) -> AdapterResult<bool> {
        let invalid = AdapterError::upstream_stream_invalid;
        if crate::protocol::completion::ResponseCompletion::from_responses_event(event_payload)
            .is_none_or(|completion| {
                completion == crate::protocol::completion::ResponseCompletion::Invalid
            })
        {
            return Err(invalid().with_parameter("response.terminal.status"));
        }
        let mut terminal_response = event_payload
            .get("response")
            .ok_or_else(|| invalid().with_parameter("response.terminal.response"))?
            .clone();
        let output = terminal_response
            .get_mut("output")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| invalid().with_parameter("response.terminal.output"))?;
        for (index, item) in output.iter_mut().enumerate() {
            self.merge_item_annotations(index as u64, item)?;
            if let Some(state) = self
                .response_items
                .get(&(index as u64))
                .filter(|state| state.closed)
            {
                snapshots::retain_completed_reasoning(&state.item, item)?;
            }
        }
        let output = terminal_response
            .get("output")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        if !self.response_items.is_empty() {
            if output.len() != self.response_items.len() {
                return Err(invalid().with_parameter("response.terminal.output_count"));
            }
            for &index in self.response_items.keys() {
                let item = output
                    .get(usize::try_from(index).map_err(|_| invalid())?)
                    .ok_or_else(|| invalid().with_parameter("response.terminal.output_index"))?;
                // Completed reasoning keeps its authoritative done ciphertext;
                // other fields still retain the completed item and every delta.
                self.validate_final_item(index, item)?;
            }
        }
        for &(output_index, _, _) in self.response_annotations.keys() {
            if usize::try_from(output_index)
                .ok()
                .is_none_or(|index| index >= output.len())
            {
                return Err(invalid().with_parameter("response.terminal.annotations"));
            }
        }
        let decoded_response = response::decode(
            WireApi::Responses,
            &terminal_response,
            &self.decoded_response.id,
        )
        .map_err(|error| {
            if error.parameter().is_some() {
                error
            } else {
                error.with_parameter("response.terminal.decode")
            }
        })?;
        if event_payload.get("type").and_then(Value::as_str) != Some("response.done")
            && event_payload.get("type").and_then(Value::as_str)
                != Some(
                    if matches!(decoded_response.finish, Finish::Length | Finish::Filter) {
                        "response.incomplete"
                    } else {
                        "response.completed"
                    },
                )
        {
            return Err(invalid().with_parameter("response.terminal.status"));
        }
        self.finish_reason = Some(decoded_response.finish);
        self.decoded_response.blocks = decoded_response.blocks;
        self.decoded_response.citations = decoded_response.citations;
        self.decoded_response.native_output = decoded_response.native_output;
        Ok(true)
    }

    fn retain_annotations(&mut self, output: u64, content: u64, part: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let Some(annotations) = part.get("annotations") else {
            return Ok(());
        };
        for (index, value) in annotations
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .enumerate()
        {
            let citation = citations::UrlCitation::parse(value)?;
            let key = (output, content, index as u64);
            if self
                .response_annotations
                .get(&key)
                .is_some_and(|previous| previous != &citation)
            {
                return Err(invalid());
            }
            self.response_annotations.insert(key, citation);
        }
        Ok(())
    }

    fn merge_item_annotations(&self, output: u64, item: &mut Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        // Compatible providers may carry citations in events or done parts
        // only. Normalize both snapshots before checking immutable done items.
        for (&(item_index, content, annotation_index), citation) in &self.response_annotations {
            if item_index != output {
                continue;
            }
            let part = item
                .pointer_mut(&format!("/content/{content}"))
                .ok_or_else(invalid)?;
            if part.get("type").and_then(Value::as_str) != Some("output_text") {
                return Err(invalid());
            }
            if part.get("annotations").is_none() {
                part["annotations"] = json!([]);
            }
            let annotations = part["annotations"].as_array_mut().ok_or_else(invalid)?;
            let index = usize::try_from(annotation_index).map_err(|_| invalid())?;
            if index == annotations.len() {
                annotations.push(citation.responses_value());
            } else if annotations
                .get(index)
                .and_then(|value| citations::UrlCitation::parse(value).ok())
                .as_ref()
                != Some(citation)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn add_annotation(&mut self, event: &Value) -> AdapterResult<()> {
        let invalid = AdapterError::upstream_stream_invalid;
        let index = |name| event.get(name).and_then(Value::as_u64).ok_or_else(invalid);
        let output = index("output_index")?;
        let content = index("content_index")?;
        let annotation = index("annotation_index")?;
        let block = self
            .indices
            .get(&format!("part:{output}:{content}"))
            .ok_or_else(invalid)?;
        self.open_response_item(event, "message")?;
        if !matches!(self.decoded_response.blocks[*block], Block::Text(_)) {
            return Err(invalid());
        }
        let citation = citations::UrlCitation::parse(event.get("annotation").ok_or_else(invalid)?)?;
        let key = (output, content, annotation);
        if self
            .response_annotations
            .get(&key)
            .is_some_and(|existing| existing != &citation)
        {
            return Err(invalid());
        }
        self.response_annotations.insert(key, citation);
        Ok(())
    }
}

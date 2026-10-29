use super::*;

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;

impl TranslationStream {
    pub(super) fn event(&mut self, event_kind: &str, mut event_payload: Value) {
        if self.translation_request.client == WireApi::Responses {
            if let Some(response) = event_payload
                .get_mut("response")
                .and_then(Value::as_object_mut)
                && !self.translation_request.metadata.is_empty()
            {
                response.insert("metadata".into(), json!(self.translation_request.metadata));
            }
            event_payload["sequence_number"] = self.sequence.into();
            self.sequence += 1;
        }
        let prefix = if event_kind.is_empty() {
            String::new()
        } else {
            format!("event: {event_kind}\n")
        };
        self.pending_frames
            .push_back(format!("{prefix}data: {event_payload}\n\n").into_bytes());
    }

    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        match self.translation_request.client {
            WireApi::Responses => self.event("response.created", json!({"type":"response.created","response":{"id":self.decoded_response.id,"object":"response","model":self.translation_request.model,"status":"in_progress","output":[]}})),
            WireApi::Messages => self.event("message_start", json!({"type":"message_start","message":{"id":self.decoded_response.id,"type":"message","role":"assistant","model":self.translation_request.model,"content":[],"stop_reason":null,"usage":response::usage_value(WireApi::Messages,&self.decoded_response.usage)}})),
            WireApi::ChatCompletions => self.chat_chunk(json!({"role":"assistant"}), None),
            WireApi::Gemini | WireApi::GeminiInteractions => {}
        }
    }

    pub(super) fn emit_changes(&mut self, upstream_finished: bool) -> AdapterResult<()> {
        self.start();
        if self.emitted.len() > self.decoded_response.blocks.len() {
            return Err(AdapterError::upstream_stream_invalid());
        }
        for block_index in 0..self.decoded_response.blocks.len() {
            let current = &self.decoded_response.blocks[block_index];
            // Search results carry provider-owned handles. Do not publish a
            // completed item, or later output, until its result is validated.
            if matches!(current, Block::WebSearch(search) if search.sources.is_none()) {
                break;
            }
            if matches!(
                current,
                Block::Text(_)
                    | Block::Reasoning(_)
                    | Block::Refusal(_)
                    | Block::ToolCall { .. }
                    | Block::WebSearch(_)
            ) {
                let previous = self.emitted.get(block_index);
                if self.translation_request.client == WireApi::Messages {
                    if block_index < self.finished_message_blocks {
                        if previous != Some(current) {
                            return Err(AdapterError::upstream_stream_invalid());
                        }
                        continue;
                    }
                    if !upstream_finished
                        && previous == Some(current)
                        && matches!(current, Block::ToolCall { .. })
                    {
                        break;
                    }
                } else if previous == Some(current) {
                    continue;
                }
            }
            if !upstream_finished
                && self.translation_request.client == WireApi::Responses
                && matches!(current, Block::ToolCall { name, .. } if self.translation_request.client_tools.get(name).is_some_and(|target| target.kind == ResponsesToolKind::ToolSearch))
            {
                break;
            }
            // Gemini patches JSON objects rather than appending JSON text.
            // Emit each assembled call once its upstream response is terminal.
            if self.translation_request.upstream == WireApi::Gemini
                && matches!(current, Block::ToolCall { .. })
                && !upstream_finished
            {
                break;
            }
            if self.emit_append(block_index, upstream_finished)? {
                if !self.finish_message_block(block_index, upstream_finished) {
                    break;
                }
                continue;
            }
            let mut block = self.decoded_response.blocks[block_index].clone();
            self.translation_request
                .normalize_function_arguments(&mut block, upstream_finished);
            if self.translation_request.client != WireApi::Messages
                && self.emitted.get(block_index) == Some(&block)
            {
                continue;
            }
            let previous_block = self.emitted.get(block_index);
            if self.translation_request.client == WireApi::Messages
                && block_index < self.finished_message_blocks
            {
                if previous_block != Some(&block) {
                    return Err(AdapterError::upstream_stream_invalid());
                }
                continue;
            }
            if previous_block.is_none() {
                if let Block::ToolCall {
                    id,
                    name,
                    arguments,
                } = &block
                {
                    if id.is_empty()
                        || name.is_empty()
                        || (arguments.is_empty() && !upstream_finished)
                    {
                        break;
                    }
                    self.translation_request.validate_tool_call(id, name)?;
                    if self.decoded_response.blocks[..block_index].iter().any(|block| {
                        matches!(block, Block::ToolCall { id: previous_id, .. } if previous_id == id)
                    }) || (!self.translation_request.parallel_tools && self.tool_index(block_index) > 0) {
                        return Err(AdapterError::upstream_stream_invalid());
                    }
                }
                self.block_start(block_index, &block)?;
            }
            // Reborrow after block_start: validation only needs the previous
            // snapshot by reference, not another copy of its growing strings.
            match (&block, self.emitted.get(block_index)) {
                (Block::WebSearch(_), None) => {}
                (Block::Refusal(text), None) => self.refusal_delta(block_index, text)?,
                (Block::Text(text), None) | (Block::Reasoning(text), None) => {
                    self.text_delta(block_index, text, matches!(block, Block::Reasoning(_)))?
                }
                (Block::ToolCall { arguments, .. }, None) => {
                    self.arguments_delta(block_index, arguments)
                }
                _ => return Err(AdapterError::upstream_stream_invalid()),
            }
            if block_index == self.emitted.len() {
                self.emitted.push(block);
            } else {
                self.emitted[block_index] = block;
            }
            if !self.finish_message_block(block_index, upstream_finished) {
                break;
            }
        }
        Ok(())
    }

    fn emit_append(&mut self, block_index: usize, complete: bool) -> AdapterResult<bool> {
        let current = &self.decoded_response.blocks[block_index];
        let (text, previous_text) = match (current, self.emitted.get(block_index)) {
            (Block::Text(text), Some(Block::Text(previous)))
            | (Block::Reasoning(text), Some(Block::Reasoning(previous)))
            | (Block::Refusal(text), Some(Block::Refusal(previous))) => {
                (std::borrow::Cow::Borrowed(text.as_str()), previous)
            }
            (
                Block::ToolCall {
                    id,
                    name,
                    arguments,
                },
                Some(Block::ToolCall {
                    id: previous_id,
                    name: previous_name,
                    arguments: previous_arguments,
                }),
            ) if id == previous_id && name == previous_name => (
                self.translation_request
                    .normalized_function_arguments(name, arguments, complete),
                previous_arguments,
            ),
            _ => return Ok(false),
        };
        // Keep the full prefix check: equal lengths alone cannot detect edits
        // to previously emitted content. Own only the new suffix across emission.
        let delta = text
            .strip_prefix(previous_text)
            .ok_or_else(AdapterError::upstream_stream_invalid)?
            .to_owned();
        if matches!(current, Block::ToolCall { .. }) {
            self.arguments_delta(block_index, &delta);
        } else if matches!(current, Block::Refusal(_)) {
            self.refusal_delta(block_index, &delta)?;
        } else {
            self.text_delta(block_index, &delta, matches!(current, Block::Reasoning(_)))?;
        }
        match &mut self.emitted[block_index] {
            Block::Text(previous)
            | Block::Reasoning(previous)
            | Block::Refusal(previous)
            | Block::ToolCall {
                arguments: previous,
                ..
            } => {
                previous.push_str(&delta);
            }
            _ => unreachable!("append-only snapshot was validated before emission"),
        }
        Ok(true)
    }

    // Returns whether the next block may be emitted.
    fn finish_message_block(&mut self, block_index: usize, upstream_finished: bool) -> bool {
        if self.translation_request.client != WireApi::Messages {
            return true;
        }
        // Messages streams one block at a time. Buffer later blocks while
        // the upstream can still add deltas to the active block.
        // A tool is executable only after the entire response validates.
        if !upstream_finished
            && (!self.closed.contains(&block_index)
                || matches!(&self.emitted[block_index], Block::ToolCall { .. }))
        {
            return false;
        }
        self.event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":block_index}),
        );
        self.finished_message_blocks += 1;
        true
    }

    fn block_start(&mut self, output_index: usize, block: &Block) -> AdapterResult<()> {
        match self.translation_request.client {
            WireApi::GeminiInteractions => {
                return Err(AdapterError::parameter_unsupported_for(
                    "gemini_interactions.native_only",
                ));
            }
            WireApi::Responses => {
                let output_item = match block {
                    Block::WebSearch(search) => search.item(false)?,
                    Block::Text(_) | Block::Refusal(_) => {
                        json!({"type":"message","id":format!("msg_{}_{output_index}",self.decoded_response.id),"role":"assistant","status":"in_progress","content":[]})
                    }
                    Block::ToolCall { id, name, .. } => {
                        let target = self.translation_request.client_tools.get(name);
                        let client_name =
                            target.map_or(name.as_str(), |target| target.name.as_str());
                        let mut item = if target
                            .is_some_and(|target| target.kind == ResponsesToolKind::ToolSearch)
                        {
                            json!({"type":"tool_search_call","id":format!("ts_{}_{output_index}",self.decoded_response.id),"call_id":id,"execution":"client","arguments":{},"status":"in_progress"})
                        } else if target
                            .is_some_and(|target| target.kind == ResponsesToolKind::Custom)
                        {
                            json!({"type":"custom_tool_call","id":super::super::super::contracts::custom_tool_item_id(id),"call_id":id,"name":client_name,"input":"","status":"in_progress"})
                        } else {
                            json!({"type":"function_call","id":format!("fc_{}_{output_index}",self.decoded_response.id),"call_id":id,"name":client_name,"arguments":"","status":"in_progress"})
                        };
                        if let Some(namespace) =
                            target.and_then(|target| target.namespace.as_deref())
                        {
                            item["namespace"] = namespace.into();
                        }
                        item
                    }
                    Block::Reasoning(_) => {
                        json!({"type":"reasoning","id":format!("rs_{}_{output_index}",self.decoded_response.id),"summary":[]})
                    }
                    _ => return Err(AdapterError::upstream_stream_invalid()),
                };
                self.event(
                    "response.output_item.added",
                    json!({"type":"response.output_item.added","output_index":output_index,"item":output_item}),
                );
                if let Block::WebSearch(search) = block {
                    for kind in [
                        "response.web_search_call.in_progress",
                        "response.web_search_call.searching",
                    ] {
                        self.event(
                            kind,
                            json!({"type":kind,"output_index":output_index,"item_id":search.id}),
                        );
                    }
                }
                if matches!(block, Block::Refusal(_)) {
                    self.event("response.content_part.added", json!({"type":"response.content_part.added","output_index":output_index,"item_id":format!("msg_{}_{output_index}",self.decoded_response.id),"content_index":0,"part":{"type":"refusal","refusal":""}}));
                } else if matches!(block, Block::Text(_)) {
                    self.event("response.content_part.added", json!({"type":"response.content_part.added","output_index":output_index,"item_id":format!("msg_{}_{output_index}",self.decoded_response.id),"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}));
                } else if matches!(block, Block::Reasoning(_)) {
                    self.event("response.reasoning_summary_part.added", json!({"type":"response.reasoning_summary_part.added","output_index":output_index,"item_id":format!("rs_{}_{output_index}",self.decoded_response.id),"summary_index":0,"part":{"type":"summary_text","text":""}}));
                }
            }
            WireApi::Messages => {
                let content = match block {
                    Block::Text(_) | Block::Refusal(_) => json!({"type":"text","text":""}),
                    // No invented Anthropic signature: native state is replayed
                    // separately to the original upstream after the tool result.
                    Block::Reasoning(_) => json!({"type":"thinking","thinking":""}),
                    Block::ToolCall { id, name, .. } => {
                        json!({"type":"tool_use","id":id,"name":name,"input":{}})
                    }
                    _ => return Err(AdapterError::upstream_stream_invalid()),
                };
                self.event(
                    "content_block_start",
                    json!({"type":"content_block_start","index":output_index,"content_block":content}),
                );
            }
            WireApi::ChatCompletions => {
                if let Block::ToolCall { id, name, .. } = block {
                    self.chat_chunk(json!({"tool_calls":[{"index":self.tool_index(output_index),"id":id,"type":"function","function":{"name":name,"arguments":""}}]}), None);
                } else if matches!(block, Block::Refusal(_)) {
                    self.chat_chunk(json!({"refusal":""}), None);
                } else if !matches!(
                    block,
                    Block::Text(_) | Block::Reasoning(_) | Block::Refusal(_)
                ) {
                    return Err(AdapterError::upstream_stream_invalid());
                }
            }
            WireApi::Gemini => {}
        }
        Ok(())
    }

    fn tool_index(&self, output_index: usize) -> usize {
        self.decoded_response.blocks[..output_index]
            .iter()
            .filter(|block| matches!(block, Block::ToolCall { .. }))
            .count()
    }

    fn refusal_delta(&mut self, output_index: usize, text: &str) -> AdapterResult<()> {
        if text.is_empty() {
            return Ok(());
        }
        match self.translation_request.client {
            WireApi::Responses => self.event("response.refusal.delta", json!({"type":"response.refusal.delta","output_index":output_index,"item_id":format!("msg_{}_{output_index}",self.decoded_response.id),"content_index":0,"delta":text})),
            WireApi::ChatCompletions => self.chat_chunk(json!({"refusal":text}), None),
            _ => self.text_delta(output_index, text, false)?,
        }
        Ok(())
    }

    fn text_delta(
        &mut self,
        output_index: usize,
        text: &str,
        reasoning: bool,
    ) -> AdapterResult<()> {
        if text.is_empty() {
            return Ok(());
        }
        match self.translation_request.client {
            WireApi::Responses if reasoning => self.event("response.reasoning_summary_text.delta", json!({"type":"response.reasoning_summary_text.delta","output_index":output_index,"item_id":format!("rs_{}_{output_index}",self.decoded_response.id),"summary_index":0,"delta":text})),
            WireApi::Responses => self.event("response.output_text.delta", json!({"type":"response.output_text.delta","output_index":output_index,"item_id":format!("msg_{}_{output_index}",self.decoded_response.id),"content_index":0,"delta":text})),
            WireApi::Messages if !reasoning => self.event("content_block_delta", json!({"type":"content_block_delta","index":output_index,"delta":{"type":"text_delta","text":text}})),
            WireApi::Messages if reasoning => self.event("content_block_delta", json!({"type":"content_block_delta","index":output_index,"delta":{"type":"thinking_delta","thinking":text}})),
            WireApi::ChatCompletions if !reasoning => self.chat_chunk(json!({"content":text}), None),
            WireApi::ChatCompletions => self.chat_chunk(json!({"reasoning_content":text}), None),
            WireApi::Gemini => self.event("", json!({"responseId":self.decoded_response.id,"modelVersion":self.translation_request.model,"candidates":[{"index":0,"content":{"role":"model","parts":[if reasoning { json!({"text":text,"thought":true}) } else { json!({"text":text}) }]}}]})),
            _ => return Err(AdapterError::upstream_stream_invalid()),
        }
        Ok(())
    }

    fn arguments_delta(&mut self, output_index: usize, arguments: &str) {
        if arguments.is_empty() {
            return;
        }
        if self.translation_request.client == WireApi::Responses
            && let Block::ToolCall { name, .. } = &self.decoded_response.blocks[output_index]
            && self
                .translation_request
                .client_tools
                .get(name)
                .is_some_and(|target| {
                    matches!(
                        target.kind,
                        ResponsesToolKind::Custom | ResponsesToolKind::ToolSearch
                    )
                })
        {
            return;
        }
        match self.translation_request.client {
            WireApi::Responses => self.event("response.function_call_arguments.delta", json!({"type":"response.function_call_arguments.delta","output_index":output_index,"item_id":format!("fc_{}_{output_index}",self.decoded_response.id),"delta":arguments})),
            WireApi::Messages => self.event("content_block_delta", json!({"type":"content_block_delta","index":output_index,"delta":{"type":"input_json_delta","partial_json":arguments}})),
            WireApi::ChatCompletions => self.chat_chunk(json!({"tool_calls":[{"index":self.tool_index(output_index),"function":{"arguments":arguments}}]}), None),
            WireApi::Gemini | WireApi::GeminiInteractions => {}
        }
    }

    fn chat_chunk(&mut self, delta: Value, finish: Option<&str>) {
        let mut chunk = json!({"id":self.decoded_response.id,"object":"chat.completion.chunk","created":0,"model":self.translation_request.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        if self.translation_request.include_stream_usage {
            chunk["usage"] = Value::Null;
        }
        self.event("", chunk);
    }

    pub(super) fn emit_end(&mut self, completed_response: &Value) -> AdapterResult<()> {
        match self.translation_request.client {
            WireApi::GeminiInteractions => {
                return Err(AdapterError::parameter_unsupported_for(
                    "gemini_interactions.native_only",
                ));
            }
            WireApi::Responses => {
                for (output_index, output_item) in completed_response
                    .get("output")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::upstream_stream_invalid)?
                    .iter()
                    .enumerate()
                {
                    match output_item.get("type").and_then(Value::as_str) {
                        Some("message") => {
                            let content_part = &output_item["content"][0];
                            if content_part.get("type").and_then(Value::as_str) == Some("refusal") {
                                self.event("response.refusal.done", json!({"type":"response.refusal.done","output_index":output_index,"item_id":output_item["id"],"content_index":0,"refusal":content_part["refusal"]}));
                            } else {
                                for (annotation_index, annotation) in content_part["annotations"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .enumerate()
                                {
                                    self.event("response.output_text.annotation.added", json!({"type":"response.output_text.annotation.added","output_index":output_index,"item_id":output_item["id"],"content_index":0,"annotation_index":annotation_index,"annotation":annotation}));
                                }
                                self.event("response.output_text.done", json!({"type":"response.output_text.done","output_index":output_index,"item_id":output_item["id"],"content_index":0,"text":content_part["text"]}));
                            }
                            self.event("response.content_part.done", json!({"type":"response.content_part.done","output_index":output_index,"item_id":output_item["id"],"content_index":0,"part":content_part}));
                        }
                        Some("function_call") => self.event("response.function_call_arguments.done", json!({"type":"response.function_call_arguments.done","output_index":output_index,"item_id":output_item["id"],"arguments":output_item["arguments"]})),
                        Some("custom_tool_call") => {
                            self.event("response.custom_tool_call_input.delta", json!({"type":"response.custom_tool_call_input.delta","output_index":output_index,"item_id":output_item["id"],"delta":output_item["input"]}));
                            self.event("response.custom_tool_call_input.done", json!({"type":"response.custom_tool_call_input.done","output_index":output_index,"item_id":output_item["id"],"input":output_item["input"]}));
                        }
                        Some("tool_search_call") => {}
                        Some("web_search_call") => {
                            self.event("response.web_search_call.completed", json!({"type":"response.web_search_call.completed","output_index":output_index,"item_id":output_item["id"]}));
                        }
                        Some("reasoning") => {
                            self.event("response.reasoning_summary_text.done", json!({"type":"response.reasoning_summary_text.done","output_index":output_index,"item_id":output_item["id"],"summary_index":0,"text":output_item["summary"][0]["text"]}));
                            self.event("response.reasoning_summary_part.done", json!({"type":"response.reasoning_summary_part.done","output_index":output_index,"item_id":output_item["id"],"summary_index":0,"part":output_item["summary"][0]}));
                        }
                        _ => return Err(AdapterError::upstream_stream_invalid()),
                    }
                    self.event("response.output_item.done", json!({"type":"response.output_item.done","output_index":output_index,"item":output_item}));
                }
                let completion_event_kind = if matches!(
                    self.decoded_response.finish,
                    Finish::Length | Finish::Filter
                ) {
                    "response.incomplete"
                } else {
                    "response.completed"
                };
                self.event(
                    completion_event_kind,
                    json!({"type":completion_event_kind,"response":completed_response}),
                );
            }
            WireApi::Messages => {
                self.event("message_delta", json!({"type":"message_delta","delta":{"stop_reason":response::finish_value(WireApi::Messages,self.decoded_response.finish),"stop_sequence":null},"usage":response::usage_value(WireApi::Messages,&self.decoded_response.usage)}));
                self.event("message_stop", json!({"type":"message_stop"}));
            }
            WireApi::ChatCompletions => {
                let annotations = citations::chat_annotations(&self.decoded_response)?;
                if !annotations.is_empty() {
                    self.chat_chunk(json!({"annotations":annotations}), None);
                }
                self.chat_chunk(
                    json!({}),
                    Some(response::finish_value(
                        WireApi::ChatCompletions,
                        self.decoded_response.finish,
                    )),
                );
                if self.translation_request.include_stream_usage {
                    self.event("", json!({"id":self.decoded_response.id,"object":"chat.completion.chunk","created":0,"model":self.translation_request.model,"choices":[],"usage":response::usage_value(WireApi::ChatCompletions,&self.decoded_response.usage)}));
                }
                self.pending_frames.push_back(b"data: [DONE]\n\n".to_vec());
            }
            WireApi::Gemini => {
                let parts = completed_response["candidates"][0]["content"]["parts"]
                    .as_array()
                    .ok_or_else(AdapterError::upstream_stream_invalid)?
                    .iter()
                    .filter(|part| part.get("functionCall").is_some())
                    .cloned()
                    .collect::<Vec<_>>();
                self.event("", json!({"responseId":self.decoded_response.id,"modelVersion":self.translation_request.model,"candidates":[{"index":0,"content":{"role":"model","parts":parts},"finishReason":response::finish_value(WireApi::Gemini,self.decoded_response.finish)}],"usageMetadata":response::usage_value(WireApi::Gemini,&self.decoded_response.usage)}));
            }
        }
        Ok(())
    }
}

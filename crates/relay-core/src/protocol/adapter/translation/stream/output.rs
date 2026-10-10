use super::*;

impl TranslationStream {
    pub(super) fn event(&mut self, event_kind: &str, mut event_payload: Value) {
        if self.translation_request.client == WireApi::Responses {
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
            WireApi::Gemini => {}
        }
    }

    pub(super) fn emit_changes(&mut self) -> AdapterResult<()> {
        self.start();
        if self.emitted.len() > self.decoded_response.blocks.len() {
            return Err(AdapterError::upstream_stream_invalid());
        }
        for block_index in 0..self.decoded_response.blocks.len() {
            let block = self.decoded_response.blocks[block_index].clone();
            let previous_block = self.emitted.get(block_index).cloned();
            if previous_block.is_none() {
                if let Block::ToolCall {
                    id,
                    name,
                    arguments,
                } = &block
                {
                    if id.is_empty() || name.is_empty() || arguments.is_empty() {
                        break;
                    }
                }
                self.block_start(block_index, &block)?;
            }
            match (&block, &previous_block) {
                (Block::Text(text), Some(Block::Text(previous_text)))
                | (Block::Reasoning(text), Some(Block::Reasoning(previous_text))) => {
                    self.text_delta(
                        block_index,
                        text.strip_prefix(previous_text)
                            .ok_or_else(AdapterError::upstream_stream_invalid)?,
                        matches!(block, Block::Reasoning(_)),
                    )?;
                }
                (Block::Text(text), None) | (Block::Reasoning(text), None) => {
                    self.text_delta(block_index, text, matches!(block, Block::Reasoning(_)))?
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
                ) if id == previous_id && name == previous_name => {
                    self.arguments_delta(
                        block_index,
                        arguments
                            .strip_prefix(previous_arguments)
                            .ok_or_else(AdapterError::upstream_stream_invalid)?,
                    );
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
        }
        Ok(())
    }

    fn block_start(&mut self, output_index: usize, block: &Block) -> AdapterResult<()> {
        match self.translation_request.client {
            WireApi::Responses => {
                let output_item = match block {
                    Block::Text(_) => {
                        json!({"type":"message","id":format!("msg_{}_{output_index}",self.decoded_response.id),"role":"assistant","status":"in_progress","content":[]})
                    }
                    Block::ToolCall { id, name, .. } => {
                        let target = self.translation_request.client_tools.get(name);
                        let client_name =
                            target.map_or(name.as_str(), |target| target.name.as_str());
                        let mut item = if target
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
                if matches!(block, Block::Text(_)) {
                    self.event("response.content_part.added", json!({"type":"response.content_part.added","output_index":output_index,"item_id":format!("msg_{}_{output_index}",self.decoded_response.id),"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}));
                } else if matches!(block, Block::Reasoning(_)) {
                    self.event("response.reasoning_summary_part.added", json!({"type":"response.reasoning_summary_part.added","output_index":output_index,"item_id":format!("rs_{}_{output_index}",self.decoded_response.id),"summary_index":0,"part":{"type":"summary_text","text":""}}));
                }
            }
            WireApi::Messages => {
                let content = match block {
                    Block::Text(_) => json!({"type":"text","text":""}),
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
                } else if !matches!(block, Block::Text(_) | Block::Reasoning(_)) {
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
        if self.translation_request.client == WireApi::Responses {
            if let Block::ToolCall { name, .. } = &self.decoded_response.blocks[output_index] {
                if self
                    .translation_request
                    .client_tools
                    .get(name)
                    .is_some_and(|target| target.kind == ResponsesToolKind::Custom)
                {
                    return;
                }
            }
        }
        match self.translation_request.client {
            WireApi::Responses => self.event("response.function_call_arguments.delta", json!({"type":"response.function_call_arguments.delta","output_index":output_index,"item_id":format!("fc_{}_{output_index}",self.decoded_response.id),"delta":arguments})),
            WireApi::Messages => self.event("content_block_delta", json!({"type":"content_block_delta","index":output_index,"delta":{"type":"input_json_delta","partial_json":arguments}})),
            WireApi::ChatCompletions => self.chat_chunk(json!({"tool_calls":[{"index":self.tool_index(output_index),"function":{"arguments":arguments}}]}), None),
            WireApi::Gemini => {}
        }
    }

    fn chat_chunk(&mut self, delta: Value, finish: Option<&str>) {
        self.event("", json!({"id":self.decoded_response.id,"object":"chat.completion.chunk","created":0,"model":self.translation_request.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]}));
    }

    pub(super) fn emit_end(&mut self, completed_response: &Value) -> AdapterResult<()> {
        match self.translation_request.client {
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
                            self.event("response.output_text.done", json!({"type":"response.output_text.done","output_index":output_index,"item_id":output_item["id"],"content_index":0,"text":content_part["text"]}));
                            self.event("response.content_part.done", json!({"type":"response.content_part.done","output_index":output_index,"item_id":output_item["id"],"content_index":0,"part":content_part}));
                        }
                        Some("function_call") => self.event("response.function_call_arguments.done", json!({"type":"response.function_call_arguments.done","output_index":output_index,"item_id":output_item["id"],"arguments":output_item["arguments"]})),
                        Some("custom_tool_call") => self.event("response.custom_tool_call_input.done", json!({"type":"response.custom_tool_call_input.done","output_index":output_index,"item_id":output_item["id"],"input":output_item["input"]})),
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
                for block_index in 0..self.decoded_response.blocks.len() {
                    self.event(
                        "content_block_stop",
                        json!({"type":"content_block_stop","index":block_index}),
                    );
                }
                self.event("message_delta", json!({"type":"message_delta","delta":{"stop_reason":response::finish_value(WireApi::Messages,self.decoded_response.finish),"stop_sequence":null},"usage":response::usage_value(WireApi::Messages,&self.decoded_response.usage)}));
                self.event("message_stop", json!({"type":"message_stop"}));
            }
            WireApi::ChatCompletions => {
                self.chat_chunk(
                    json!({}),
                    Some(response::finish_value(
                        WireApi::ChatCompletions,
                        self.decoded_response.finish,
                    )),
                );
                self.event("", json!({"id":self.decoded_response.id,"object":"chat.completion.chunk","created":0,"model":self.translation_request.model,"choices":[],"usage":response::usage_value(WireApi::ChatCompletions,&self.decoded_response.usage)}));
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

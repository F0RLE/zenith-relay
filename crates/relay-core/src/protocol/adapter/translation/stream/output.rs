use super::*;

impl TranslationStream {
    pub(super) fn event(&mut self, kind: &str, mut value: Value) {
        if self.request.client == WireApi::Responses {
            value["sequence_number"] = self.sequence.into();
            self.sequence += 1;
        }
        let prefix = if kind.is_empty() {
            String::new()
        } else {
            format!("event: {kind}\n")
        };
        self.output
            .push_back(format!("{prefix}data: {value}\n\n").into_bytes());
    }

    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        match self.request.client {
            WireApi::Responses => self.event("response.created", json!({"type":"response.created","response":{"id":self.response.id,"object":"response","model":self.request.model,"status":"in_progress","output":[]}})),
            WireApi::Messages => self.event("message_start", json!({"type":"message_start","message":{"id":self.response.id,"type":"message","role":"assistant","model":self.request.model,"content":[],"stop_reason":null,"usage":response::usage_value(WireApi::Messages,&self.response.usage)}})),
            WireApi::ChatCompletions => self.chat_chunk(json!({"role":"assistant"}), None),
            WireApi::Gemini => {}
        }
    }

    pub(super) fn emit_changes(&mut self) -> AdapterResult<()> {
        self.start();
        if self.emitted.len() > self.response.blocks.len() {
            return Err(AdapterError::upstream_stream_invalid());
        }
        for index in 0..self.response.blocks.len() {
            let block = self.response.blocks[index].clone();
            let previous = self.emitted.get(index).cloned();
            if previous.is_none() {
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
                self.block_start(index, &block)?;
            }
            match (&block, &previous) {
                (Block::Text(text), Some(Block::Text(old)))
                | (Block::Reasoning(text), Some(Block::Reasoning(old))) => {
                    self.text_delta(
                        index,
                        text.strip_prefix(old)
                            .ok_or_else(AdapterError::upstream_stream_invalid)?,
                        matches!(block, Block::Reasoning(_)),
                    )?;
                }
                (Block::Text(text), None) | (Block::Reasoning(text), None) => {
                    self.text_delta(index, text, matches!(block, Block::Reasoning(_)))?
                }
                (
                    Block::ToolCall {
                        id,
                        name,
                        arguments,
                    },
                    Some(Block::ToolCall {
                        id: old_id,
                        name: old_name,
                        arguments: old_args,
                    }),
                ) if id == old_id && name == old_name => {
                    self.arguments_delta(
                        index,
                        arguments
                            .strip_prefix(old_args)
                            .ok_or_else(AdapterError::upstream_stream_invalid)?,
                    );
                }
                (Block::ToolCall { arguments, .. }, None) => self.arguments_delta(index, arguments),
                _ => return Err(AdapterError::upstream_stream_invalid()),
            }
            if index == self.emitted.len() {
                self.emitted.push(block);
            } else {
                self.emitted[index] = block;
            }
        }
        Ok(())
    }

    fn block_start(&mut self, index: usize, block: &Block) -> AdapterResult<()> {
        match self.request.client {
            WireApi::Responses => {
                let item = match block {
                    Block::Text(_) => {
                        json!({"type":"message","id":format!("msg_{}_{index}",self.response.id),"role":"assistant","status":"in_progress","content":[]})
                    }
                    Block::ToolCall { id, name, .. } => {
                        json!({"type":"function_call","id":format!("fc_{}_{index}",self.response.id),"call_id":id,"name":name,"arguments":"","status":"in_progress"})
                    }
                    Block::Reasoning(_) => {
                        json!({"type":"reasoning","id":format!("rs_{}_{index}",self.response.id),"summary":[]})
                    }
                    _ => return Err(AdapterError::upstream_stream_invalid()),
                };
                self.event(
                    "response.output_item.added",
                    json!({"type":"response.output_item.added","output_index":index,"item":item}),
                );
                if matches!(block, Block::Text(_)) {
                    self.event("response.content_part.added", json!({"type":"response.content_part.added","output_index":index,"item_id":format!("msg_{}_{index}",self.response.id),"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}));
                } else if matches!(block, Block::Reasoning(_)) {
                    self.event("response.reasoning_summary_part.added", json!({"type":"response.reasoning_summary_part.added","output_index":index,"item_id":format!("rs_{}_{index}",self.response.id),"summary_index":0,"part":{"type":"summary_text","text":""}}));
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
                    json!({"type":"content_block_start","index":index,"content_block":content}),
                );
            }
            WireApi::ChatCompletions => {
                if let Block::ToolCall { id, name, .. } = block {
                    self.chat_chunk(json!({"tool_calls":[{"index":self.tool_index(index),"id":id,"type":"function","function":{"name":name,"arguments":""}}]}), None);
                } else if !matches!(block, Block::Text(_) | Block::Reasoning(_)) {
                    return Err(AdapterError::upstream_stream_invalid());
                }
            }
            WireApi::Gemini => {}
        }
        Ok(())
    }

    fn tool_index(&self, index: usize) -> usize {
        self.response.blocks[..index]
            .iter()
            .filter(|block| matches!(block, Block::ToolCall { .. }))
            .count()
    }

    fn text_delta(&mut self, index: usize, text: &str, reasoning: bool) -> AdapterResult<()> {
        if text.is_empty() {
            return Ok(());
        }
        match self.request.client {
            WireApi::Responses if reasoning => self.event("response.reasoning_summary_text.delta", json!({"type":"response.reasoning_summary_text.delta","output_index":index,"item_id":format!("rs_{}_{index}",self.response.id),"summary_index":0,"delta":text})),
            WireApi::Responses => self.event("response.output_text.delta", json!({"type":"response.output_text.delta","output_index":index,"item_id":format!("msg_{}_{index}",self.response.id),"content_index":0,"delta":text})),
            WireApi::Messages if !reasoning => self.event("content_block_delta", json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}})),
            WireApi::Messages if reasoning => self.event("content_block_delta", json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":text}})),
            WireApi::ChatCompletions if !reasoning => self.chat_chunk(json!({"content":text}), None),
            WireApi::ChatCompletions => self.chat_chunk(json!({"reasoning_content":text}), None),
            WireApi::Gemini => self.event("", json!({"responseId":self.response.id,"modelVersion":self.request.model,"candidates":[{"index":0,"content":{"role":"model","parts":[if reasoning { json!({"text":text,"thought":true}) } else { json!({"text":text}) }]}}]})),
            _ => return Err(AdapterError::upstream_stream_invalid()),
        }
        Ok(())
    }

    fn arguments_delta(&mut self, index: usize, arguments: &str) {
        if arguments.is_empty() {
            return;
        }
        match self.request.client {
            WireApi::Responses => self.event("response.function_call_arguments.delta", json!({"type":"response.function_call_arguments.delta","output_index":index,"item_id":format!("fc_{}_{index}",self.response.id),"delta":arguments})),
            WireApi::Messages => self.event("content_block_delta", json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":arguments}})),
            WireApi::ChatCompletions => self.chat_chunk(json!({"tool_calls":[{"index":self.tool_index(index),"function":{"arguments":arguments}}]}), None),
            WireApi::Gemini => {}
        }
    }

    fn chat_chunk(&mut self, delta: Value, finish: Option<&str>) {
        self.event("", json!({"id":self.response.id,"object":"chat.completion.chunk","created":0,"model":self.request.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]}));
    }

    pub(super) fn emit_end(&mut self, completed: &Value) -> AdapterResult<()> {
        match self.request.client {
            WireApi::Responses => {
                for (index, item) in completed
                    .get("output")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::upstream_stream_invalid)?
                    .iter()
                    .enumerate()
                {
                    match item.get("type").and_then(Value::as_str) {
                        Some("message") => {
                            let part = &item["content"][0];
                            self.event("response.output_text.done", json!({"type":"response.output_text.done","output_index":index,"item_id":item["id"],"content_index":0,"text":part["text"]}));
                            self.event("response.content_part.done", json!({"type":"response.content_part.done","output_index":index,"item_id":item["id"],"content_index":0,"part":part}));
                        }
                        Some("function_call") => self.event("response.function_call_arguments.done", json!({"type":"response.function_call_arguments.done","output_index":index,"item_id":item["id"],"arguments":item["arguments"]})),
                        Some("reasoning") => {
                            self.event("response.reasoning_summary_text.done", json!({"type":"response.reasoning_summary_text.done","output_index":index,"item_id":item["id"],"summary_index":0,"text":item["summary"][0]["text"]}));
                            self.event("response.reasoning_summary_part.done", json!({"type":"response.reasoning_summary_part.done","output_index":index,"item_id":item["id"],"summary_index":0,"part":item["summary"][0]}));
                        }
                        _ => return Err(AdapterError::upstream_stream_invalid()),
                    }
                    self.event("response.output_item.done", json!({"type":"response.output_item.done","output_index":index,"item":item}));
                }
                let kind = if matches!(self.response.finish, Finish::Length | Finish::Filter) {
                    "response.incomplete"
                } else {
                    "response.completed"
                };
                self.event(kind, json!({"type":kind,"response":completed}));
            }
            WireApi::Messages => {
                for index in 0..self.response.blocks.len() {
                    self.event(
                        "content_block_stop",
                        json!({"type":"content_block_stop","index":index}),
                    );
                }
                self.event("message_delta", json!({"type":"message_delta","delta":{"stop_reason":response::finish_value(WireApi::Messages,self.response.finish),"stop_sequence":null},"usage":response::usage_value(WireApi::Messages,&self.response.usage)}));
                self.event("message_stop", json!({"type":"message_stop"}));
            }
            WireApi::ChatCompletions => {
                self.chat_chunk(
                    json!({}),
                    Some(response::finish_value(
                        WireApi::ChatCompletions,
                        self.response.finish,
                    )),
                );
                self.event("", json!({"id":self.response.id,"object":"chat.completion.chunk","created":0,"model":self.request.model,"choices":[],"usage":response::usage_value(WireApi::ChatCompletions,&self.response.usage)}));
                self.output.push_back(b"data: [DONE]\n\n".to_vec());
            }
            WireApi::Gemini => {
                let parts = completed["candidates"][0]["content"]["parts"]
                    .as_array()
                    .ok_or_else(AdapterError::upstream_stream_invalid)?
                    .iter()
                    .filter(|part| part.get("functionCall").is_some())
                    .cloned()
                    .collect::<Vec<_>>();
                self.event("", json!({"responseId":self.response.id,"modelVersion":self.request.model,"candidates":[{"index":0,"content":{"role":"model","parts":parts},"finishReason":response::finish_value(WireApi::Gemini,self.response.finish)}],"usageMetadata":response::usage_value(WireApi::Gemini,&self.response.usage)}));
            }
        }
        Ok(())
    }
}

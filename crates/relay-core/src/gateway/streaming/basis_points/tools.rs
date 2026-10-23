use super::{Value, MAX_TOOLS};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct NativeTools {
    calls: HashMap<String, Option<String>>,
}

impl NativeTools {
    fn remember(&mut self, id: &str, call_id: Option<&str>) -> Result<(), &'static str> {
        if id.is_empty() || (!self.calls.contains_key(id) && self.calls.len() >= MAX_TOOLS) {
            return Err("stream.tool.identity");
        }
        let recorded = self.calls.entry(id.to_owned()).or_default();
        if let Some(call_id) = call_id {
            if call_id.is_empty() || recorded.as_deref().is_some_and(|before| before != call_id) {
                return Err("stream.tool.identity");
            }
            *recorded = Some(call_id.to_owned());
        }
        Ok(())
    }

    pub(super) fn observe(&mut self, event: &Value) -> Result<Option<usize>, &'static str> {
        let kind = event["type"].as_str().unwrap_or_default();
        let arguments = kind.starts_with("response.function_call_arguments.")
            || kind.starts_with("response.custom_tool_call_input.");
        let tool = matches!(
            event["item"]["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        );
        if !arguments && !tool {
            return Ok(None);
        }
        let index = match event.get("output_index") {
            Some(index) => index.as_u64().ok_or("stream.output_index")?,
            None => 0,
        };
        let index = usize::try_from(index).map_err(|_| "stream.output_index")?;
        if index >= MAX_TOOLS {
            return Err("stream.output_index");
        }
        if arguments {
            self.remember(
                event["item_id"].as_str().ok_or("stream.tool.item_id")?,
                None,
            )?;
        } else {
            if !matches!(
                kind,
                "response.output_item.added" | "response.output_item.done"
            ) {
                return Err("stream.tool.event");
            }
            let call_id = event["item"]["call_id"].as_str();
            if kind == "response.output_item.done" && call_id.is_none() {
                return Err("stream.tool.call_id");
            }
            self.remember(
                event["item"]["id"].as_str().ok_or("stream.tool.item_id")?,
                call_id,
            )?;
        }
        Ok(Some(index))
    }

    pub(super) fn complete(&mut self, items: &[Value]) -> Result<(), &'static str> {
        for item in items.iter().filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        }) {
            if let Some(id) = item["id"].as_str() {
                if let Some(recorded) = self.calls.remove(id).flatten() {
                    if item["call_id"].as_str() != Some(recorded.as_str()) {
                        return Err("stream.tool.call_id");
                    }
                }
            }
        }
        if !self.calls.is_empty() || items.len() > MAX_TOOLS {
            return Err("stream.tool.output");
        }
        Ok(())
    }
}

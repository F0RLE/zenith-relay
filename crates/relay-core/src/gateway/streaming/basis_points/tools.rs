use super::{MAX_TOOLS, Value};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct NativeTools {
    calls: HashMap<String, Option<String>>,
}

impl NativeTools {
    fn remember(&mut self, id: &str, call_id: Option<&str>) -> Result<(), &'static str> {
        if id.is_empty() || call_id == Some("") {
            return Err("stream.tool.identity");
        }
        if let Some(recorded) = self.calls.get_mut(id) {
            if let Some(call_id) = call_id {
                match recorded {
                    Some(before) if before.as_str() != call_id => {
                        return Err("stream.tool.identity");
                    }
                    Some(_) => {}
                    None => *recorded = Some(call_id.to_owned()),
                }
            }
            return Ok(());
        }
        if self.calls.len() >= MAX_TOOLS {
            return Err("stream.tool.identity");
        }
        self.calls.insert(id.to_owned(), call_id.map(str::to_owned));
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
            if let Some(id) = item["id"].as_str()
                && let Some(recorded) = self.calls.remove(id).flatten()
                && item["call_id"].as_str() != Some(recorded.as_str())
            {
                return Err("stream.tool.call_id");
            }
        }
        if !self.calls.is_empty() || items.len() > MAX_TOOLS {
            return Err("stream.tool.output");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_fragments_reuse_identity_storage() {
        let mut tools = NativeTools::default();
        tools.remember("fc_test", None).unwrap();
        tools.remember("fc_test", Some("call_test")).unwrap();
        let (key, call) = tools.calls.get_key_value("fc_test").unwrap();
        let key_pointer = key.as_ptr();
        let call_pointer = call.as_ref().unwrap().as_ptr();
        for _ in 0..128 {
            tools.remember("fc_test", None).unwrap();
            tools.remember("fc_test", Some("call_test")).unwrap();
            let (key, call) = tools.calls.get_key_value("fc_test").unwrap();
            assert_eq!(key.as_ptr(), key_pointer);
            assert_eq!(call.as_ref().unwrap().as_ptr(), call_pointer);
        }
        assert_eq!(tools.calls.len(), 1);
        assert_eq!(
            tools.remember("fc_test", Some("changed")),
            Err("stream.tool.identity")
        );
        assert_eq!(
            tools.remember("fc_test", Some("")),
            Err("stream.tool.identity")
        );
        assert_eq!(tools.remember("", None), Err("stream.tool.identity"));
        tools
            .complete(&[serde_json::json!({
                "type":"function_call","id":"fc_test","call_id":"call_test"
            })])
            .unwrap();
        assert!(tools.calls.is_empty());
    }

    #[test]
    fn identity_limit_rejects_new_calls_not_known_fragments() {
        let mut tools = NativeTools::default();
        for index in 0..MAX_TOOLS {
            tools.remember(&format!("fc_{index}"), None).unwrap();
        }
        assert_eq!(
            tools.remember("overflow", None),
            Err("stream.tool.identity")
        );
        tools.remember("fc_0", Some("call_0")).unwrap();
        tools.remember("fc_0", None).unwrap();
        tools.remember("fc_0", Some("call_0")).unwrap();
        assert_eq!(tools.calls.len(), MAX_TOOLS);
        assert_eq!(tools.complete(&[]), Err("stream.tool.output"));
    }
}

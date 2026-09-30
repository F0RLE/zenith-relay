use super::*;
use crate::sources::WireApi;
use crate::CacheWriteTtl;
use serde_json::{json, Value};
mod message_bridge;
mod native_request;
mod replay;
mod tool_bridge;

fn request(input: Value) -> Value {
    json!({
        "model": "claude-test",
        "input": input,
        "tools": [{
            "type": "function",
            "name": "run_command",
            "description": "Run a command",
            "parameters": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }
        }]
    })
}

use super::*;
use serde_json::{json, Value};

mod preparation;
mod tool_responses;

fn request_with_tool() -> Value {
    json!({
        "model": "gpt-6-astra",
        "input": "Inspect the repo",
        "tools": [{"type":"function","name":"exec_command","description":"Run a command","parameters":{"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"]}}]
    })
}

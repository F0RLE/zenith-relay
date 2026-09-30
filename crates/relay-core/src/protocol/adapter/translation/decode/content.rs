use super::super::{
    checked, optional_bool, required_text, AdapterError, AdapterResult, Block, Role,
};
use crate::url_has_userinfo;
use crate::WireApi;
use serde_json::{json, Value};

mod request;
mod tools;

pub(super) use request::{choice, common, output_format};
pub(super) use tools::tools;

pub(super) fn role(value: &Value) -> AdapterResult<Role> {
    match value.get("role").and_then(Value::as_str) {
        Some("system" | "developer") => Ok(Role::System),
        Some("user") | None => Ok(Role::User),
        Some("assistant" | "model") => Ok(Role::Assistant),
        _ => Err(AdapterError::invalid_request()),
    }
}

pub(super) fn text_blocks(value: &Value, protocol: WireApi) -> AdapterResult<Vec<Block>> {
    if let Some(text) = value.as_str() {
        return Ok(vec![Block::Text(text.to_owned())]);
    }
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?
        .iter()
        .enumerate()
        .map(|(index, part)| block(part, protocol, index))
        .collect()
}

pub(super) fn block(part: &Value, protocol: WireApi, _index: usize) -> AdapterResult<Block> {
    if protocol == WireApi::Gemini {
        return gemini_block(part);
    }
    match required_text(part, "type")? {
        "thinking" if protocol == WireApi::Messages => {
            checked(part, &["type", "thinking"])?;
            Ok(Block::Reasoning(
                part.get("thinking")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ))
        }
        "text" | "input_text" | "output_text" => {
            checked(part, &["type", "text", "annotations"])?;
            if part
                .get("annotations")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
            {
                return Err(AdapterError::parameter_unsupported());
            }
            Ok(Block::Text(
                part.get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ))
        }
        "image_url" | "input_image" | "image" => image_block(part),
        "tool_use" => {
            checked(part, &["type", "id", "name", "input"])?;
            Ok(Block::ToolCall {
                id: required_text(part, "id")?.into(),
                name: required_text(part, "name")?.into(),
                arguments: part
                    .get("input")
                    .filter(|input| input.is_object())
                    .ok_or_else(AdapterError::invalid_request)?
                    .to_string(),
            })
        }
        "tool_result" => {
            checked(part, &["type", "tool_use_id", "content", "is_error"])?;
            Ok(Block::ToolResult {
                id: required_text(part, "tool_use_id")?.into(),
                name: String::new(),
                content: plain_text(part.get("content").unwrap_or(&Value::Null), protocol)?,
                is_error: optional_bool(part, "is_error")?.unwrap_or(false),
            })
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}

fn gemini_block(part: &Value) -> AdapterResult<Block> {
    checked(
        part,
        &[
            "text",
            "inlineData",
            "fileData",
            "functionCall",
            "functionResponse",
            "thought",
        ],
    )?;
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return Ok(
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                Block::Reasoning(text.into())
            } else {
                Block::Text(text.into())
            },
        );
    }
    if let Some(data) = part.get("inlineData") {
        checked(data, &["mimeType", "data"])?;
        return image(
            format!(
                "data:{};base64,{}",
                required_text(data, "mimeType")?,
                required_text(data, "data")?
            ),
            None,
        );
    }
    if let Some(data) = part.get("fileData") {
        checked(data, &["mimeType", "fileUri"])?;
        if data
            .get("mimeType")
            .and_then(Value::as_str)
            .is_none_or(|mime| !mime.starts_with("image/"))
        {
            return Err(AdapterError::parameter_unsupported());
        }
        return image(required_text(data, "fileUri")?.into(), None);
    }
    if let Some(call) = part.get("functionCall") {
        checked(call, &["id", "name", "args"])?;
        return Ok(Block::ToolCall {
            id: call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default(),
            name: required_text(call, "name")?.into(),
            arguments: call.get("args").unwrap_or(&json!({})).to_string(),
        });
    }
    if let Some(result) = part.get("functionResponse") {
        checked(result, &["id", "name", "response"])?;
        let response = result
            .get("response")
            .ok_or_else(AdapterError::invalid_request)?;
        return Ok(Block::ToolResult {
            id: result
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            name: required_text(result, "name")?.into(),
            content: response.to_string(),
            is_error: response.get("error").is_some(),
        });
    }
    Err(AdapterError::parameter_unsupported())
}

fn image_block(part: &Value) -> AdapterResult<Block> {
    match required_text(part, "type")? {
        "image_url" | "input_image" => {
            checked(part, &["type", "image_url", "detail"])?;
            let image_value = part
                .get("image_url")
                .ok_or_else(AdapterError::invalid_request)?;
            let (url, detail) = if let Some(url) = image_value.as_str() {
                (url, part.get("detail").and_then(Value::as_str))
            } else {
                checked(image_value, &["url", "detail"])?;
                (
                    required_text(image_value, "url")?,
                    image_value.get("detail").and_then(Value::as_str),
                )
            };
            image(url.into(), detail.map(str::to_owned))
        }
        "image" => {
            checked(part, &["type", "source"])?;
            let source = part
                .get("source")
                .ok_or_else(AdapterError::invalid_request)?;
            match required_text(source, "type")? {
                "base64" => {
                    checked(source, &["type", "media_type", "data"])?;
                    image(
                        format!(
                            "data:{};base64,{}",
                            required_text(source, "media_type")?,
                            required_text(source, "data")?
                        ),
                        None,
                    )
                }
                "url" => {
                    checked(source, &["type", "url"])?;
                    image(required_text(source, "url")?.into(), None)
                }
                _ => Err(AdapterError::parameter_unsupported()),
            }
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}

pub(super) fn image(url: String, detail: Option<String>) -> AdapterResult<Block> {
    use base64::Engine;
    if let Some(data) = url.strip_prefix("data:") {
        let (mime, data) = data
            .split_once(";base64,")
            .ok_or_else(AdapterError::invalid_request)?;
        if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&mime)
            || data.len() > 28 * 1024 * 1024
        {
            return Err(AdapterError::parameter_unsupported());
        }
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| AdapterError::invalid_request())?;
    } else {
        let parsed = url::Url::parse(&url).map_err(|_| AdapterError::invalid_request())?;
        if !matches!(parsed.scheme(), "https" | "http") || url_has_userinfo(&parsed) {
            return Err(AdapterError::parameter_unsupported());
        }
    }
    Ok(Block::Image { url, detail })
}

pub(super) fn plain_text(value: &Value, protocol: WireApi) -> AdapterResult<String> {
    let mut result = String::new();
    for block in text_blocks(value, protocol)? {
        let Block::Text(text) = block else {
            return Err(AdapterError::parameter_unsupported());
        };
        result.push_str(&text);
    }
    Ok(result)
}

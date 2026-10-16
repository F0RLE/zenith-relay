use super::super::{
    checked, optional_bool, required_text, AdapterError, AdapterResult, Block, Role,
};
use crate::url_has_userinfo;
use crate::WireApi;
use serde_json::{json, Value};

mod request;
mod tools;

pub(super) use request::{choice, common, output_format, sampling_extensions};
pub(super) use tools::{responses_tools, tools};

/// Gemini's OpenAPI and JSON Schema fields are mutually exclusive. Resolve
/// them once without silently preferring one set of constraints over another.
pub(super) fn schema_field(
    object: &Value,
    json_schema: &str,
    openapi_schema: &str,
) -> AdapterResult<Option<Value>> {
    match (object.get(json_schema), object.get(openapi_schema)) {
        (Some(_), Some(_)) => Err(AdapterError::invalid_request()),
        (Some(schema), None) if schema.is_object() => Ok(Some(schema.clone())),
        (None, Some(schema)) if schema.is_object() => {
            super::super::schema::from_gemini(schema).map(Some)
        }
        (None, None) => Ok(None),
        _ => Err(AdapterError::invalid_request()),
    }
}

pub(super) fn role(message_part: &Value) -> AdapterResult<Role> {
    match message_part.get("role").and_then(Value::as_str) {
        Some("system" | "developer") => Ok(Role::System),
        Some("user") | None => Ok(Role::User),
        Some("assistant" | "model") => Ok(Role::Assistant),
        _ => Err(AdapterError::invalid_request()),
    }
}

pub(super) fn text_blocks(content_value: &Value, protocol: WireApi) -> AdapterResult<Vec<Block>> {
    if let Some(text) = content_value.as_str() {
        return Ok(vec![Block::Text(text.to_owned())]);
    }
    if content_value.is_null() {
        return Ok(Vec::new());
    }
    content_value
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?
        .iter()
        .enumerate()
        .map(|(index, content_part)| block(content_part, protocol, index))
        .collect()
}

pub(super) fn block(
    content_part: &Value,
    protocol: WireApi,
    _index: usize,
) -> AdapterResult<Block> {
    if protocol == WireApi::Gemini {
        return gemini_block(content_part);
    }
    match required_text(content_part, "type")? {
        "thinking" if protocol == WireApi::Messages => {
            checked(content_part, &["type", "thinking", "signature"])?;
            // Messages clients initialize unsigned, translated summaries with
            // signature: "" while assembling a stream. It contains no native
            // state. Real signatures still need their originating native route;
            // the portable summary cannot replace our scoped continuation.
            if let Some(signature) = content_part
                .get("signature")
                .filter(|value| !value.is_null())
            {
                let parameter = "messages[].content[].signature";
                let signature = signature
                    .as_str()
                    .ok_or_else(|| AdapterError::invalid_request().with_parameter(parameter))?;
                if !signature.is_empty() {
                    return Err(AdapterError::parameter_unsupported_for(parameter));
                }
            }
            Ok(Block::Reasoning(
                content_part
                    .get("thinking")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ))
        }
        "text" | "input_text" | "output_text" => {
            content_fields(content_part, protocol, &["type", "text", "annotations"])?;
            if content_part
                .get("annotations")
                .and_then(Value::as_array)
                .is_some_and(|annotation_items| !annotation_items.is_empty())
            {
                return Err(AdapterError::parameter_unsupported());
            }
            Ok(Block::Text(
                content_part
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ))
        }
        "refusal" if protocol == WireApi::Responses => {
            checked(content_part, &["type", "refusal"])?;
            Ok(Block::Refusal(
                content_part
                    .get("refusal")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ))
        }
        "image_url" | "input_image" | "image" => image_block(content_part, protocol),
        "tool_use" => {
            content_fields(content_part, protocol, &["type", "id", "name", "input"])?;
            Ok(Block::ToolCall {
                id: required_text(content_part, "id")?.into(),
                name: required_text(content_part, "name")?.into(),
                arguments: content_part
                    .get("input")
                    .filter(|tool_input| tool_input.is_object())
                    .ok_or_else(AdapterError::invalid_request)?
                    .to_string(),
            })
        }
        "tool_result" => {
            content_fields(
                content_part,
                protocol,
                &["type", "tool_use_id", "content", "is_error"],
            )?;
            Ok(Block::ToolResult {
                id: required_text(content_part, "tool_use_id")?.into(),
                name: String::new(),
                content: tool_result_blocks(
                    content_part.get("content").unwrap_or(&Value::Null),
                    protocol,
                )?,
                is_error: optional_bool(content_part, "is_error")?.unwrap_or(false),
                raw_text: false,
            })
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}

fn gemini_block(gemini_part: &Value) -> AdapterResult<Block> {
    checked(
        gemini_part,
        &[
            "text",
            "inlineData",
            "fileData",
            "functionCall",
            "functionResponse",
            "thought",
        ],
    )?;
    if let Some(text) = gemini_part.get("text").and_then(Value::as_str) {
        return Ok(
            if gemini_part.get("thought").and_then(Value::as_bool) == Some(true) {
                Block::Reasoning(text.into())
            } else {
                Block::Text(text.into())
            },
        );
    }
    if let Some(inline_data) = gemini_part.get("inlineData") {
        checked(inline_data, &["mimeType", "data"])?;
        return image(
            format!(
                "data:{};base64,{}",
                required_text(inline_data, "mimeType")?,
                required_text(inline_data, "data")?
            ),
            None,
        );
    }
    if let Some(file_data) = gemini_part.get("fileData") {
        checked(file_data, &["mimeType", "fileUri"])?;
        if file_data
            .get("mimeType")
            .and_then(Value::as_str)
            .is_none_or(|mime| !mime.starts_with("image/"))
        {
            return Err(AdapterError::parameter_unsupported());
        }
        return image(required_text(file_data, "fileUri")?.into(), None);
    }
    if let Some(function_call) = gemini_part.get("functionCall") {
        checked(function_call, &["id", "name", "args"])?;
        return Ok(Block::ToolCall {
            id: function_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default(),
            name: required_text(function_call, "name")?.into(),
            arguments: function_call.get("args").unwrap_or(&json!({})).to_string(),
        });
    }
    if let Some(function_response) = gemini_part.get("functionResponse") {
        checked(function_response, &["id", "name", "response", "parts"])?;
        let response_payload = function_response
            .get("response")
            .filter(|response| response.is_object())
            .ok_or_else(AdapterError::invalid_request)?;
        let mut content = vec![Block::Text(response_payload.to_string())];
        if let Some(parts) = function_response
            .get("parts")
            .filter(|parts| !parts.is_null())
        {
            for part in parts.as_array().ok_or_else(AdapterError::invalid_request)? {
                checked(part, &["inlineData"])?;
                let media = gemini_block(part)?;
                if !matches!(media, Block::Image { .. }) {
                    return Err(AdapterError::parameter_unsupported());
                }
                content.push(media);
            }
        }
        return Ok(Block::ToolResult {
            id: function_response
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            name: required_text(function_response, "name")?.into(),
            content,
            is_error: response_payload.get("error").is_some(),
            raw_text: false,
        });
    }
    Err(AdapterError::parameter_unsupported())
}

fn content_fields(container: &Value, protocol: WireApi, allowed: &[&str]) -> AdapterResult<()> {
    if protocol == WireApi::Messages {
        super::super::options::messages_fields(container, allowed)
    } else {
        checked(container, allowed)
    }
}

fn image_block(image_part: &Value, protocol: WireApi) -> AdapterResult<Block> {
    match required_text(image_part, "type")? {
        "image_url" | "input_image" => {
            checked(image_part, &["type", "image_url", "detail"])?;
            let image_value = image_part
                .get("image_url")
                .ok_or_else(AdapterError::invalid_request)?;
            let (url, detail) = if let Some(url) = image_value.as_str() {
                (url, image_part.get("detail").and_then(Value::as_str))
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
            content_fields(image_part, protocol, &["type", "source"])?;
            let image_source = image_part
                .get("source")
                .ok_or_else(AdapterError::invalid_request)?;
            match required_text(image_source, "type")? {
                "base64" => {
                    checked(image_source, &["type", "media_type", "data"])?;
                    image(
                        format!(
                            "data:{};base64,{}",
                            required_text(image_source, "media_type")?,
                            required_text(image_source, "data")?
                        ),
                        None,
                    )
                }
                "url" => {
                    checked(image_source, &["type", "url"])?;
                    image(required_text(image_source, "url")?.into(), None)
                }
                _ => Err(AdapterError::parameter_unsupported()),
            }
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}

pub(super) fn image(url: String, detail: Option<String>) -> AdapterResult<Block> {
    use base64::Engine;
    if let Some(base64_payload) = url.strip_prefix("data:") {
        let (mime, base64_data) = base64_payload
            .split_once(";base64,")
            .ok_or_else(AdapterError::invalid_request)?;
        if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&mime) {
            return Err(AdapterError::parameter_unsupported());
        }
        base64::engine::general_purpose::STANDARD
            .decode(base64_data)
            .map_err(|_| AdapterError::invalid_request())?;
    } else {
        let parsed = url::Url::parse(&url).map_err(|_| AdapterError::invalid_request())?;
        if !matches!(parsed.scheme(), "https" | "http") || url_has_userinfo(&parsed) {
            return Err(AdapterError::parameter_unsupported());
        }
    }
    Ok(Block::Image { url, detail })
}

pub(super) fn tool_result_blocks(
    content_value: &Value,
    protocol: WireApi,
) -> AdapterResult<Vec<Block>> {
    if protocol == WireApi::Responses
        && match content_value {
            Value::Array(items) => !items.iter().any(|item| item.get("type").is_some()),
            Value::Object(_) | Value::Bool(_) | Value::Number(_) => true,
            _ => false,
        }
    {
        return Ok(vec![Block::Text(content_value.to_string())]);
    }
    let blocks = text_blocks(content_value, protocol)?;
    for block in &blocks {
        if !matches!(block, Block::Text(_) | Block::Image { .. }) {
            return Err(AdapterError::parameter_unsupported());
        }
    }
    Ok(blocks)
}

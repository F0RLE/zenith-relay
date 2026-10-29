//! Locally validated non-strict output, never native strict decoding.
use super::schema::validator;
use crate::protocol::AdapterError;
use serde_json::Value;
use std::sync::Arc;

pub(super) const OPTION: &str = "relay_bps_output_validation";
const MAX_SCHEMA_BYTES: usize = 1024 * 1024;
const MAX_ANSWER_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct StructuredOutput {
    format: Value,
    schema: Option<Arc<jsonschema::Validator>>,
}

pub(in crate::gateway::execution) fn requires_buffering(request: &Value) -> bool {
    match request.get(OPTION) {
        Some(Value::Bool(true)) => true,
        None => {
            request.pointer("/text/format/type").and_then(Value::as_str) == Some("json_schema")
                && request.pointer("/text/format/strict") == Some(&Value::Bool(false))
        }
        _ => false,
    }
}

pub(super) fn prepare(request: &Value) -> Result<Option<StructuredOutput>, AdapterError> {
    if request.get(OPTION).is_some_and(|value| !value.is_boolean()) {
        return Err(AdapterError::invalid_request().with_parameter(OPTION));
    }
    // An explicit non-strict schema can use the existing local validator.
    // Omitted strictness, strict requests, and explicit opt-outs do not silently
    // acquire this compatibility mode. Admission and both stream paths share it.
    if !requires_buffering(request) {
        return Ok(None);
    }
    let Some(format) = request.pointer("/text/format") else {
        return Ok(None);
    };
    let fields = format
        .as_object()
        .ok_or_else(|| AdapterError::invalid_request().with_parameter("text.format"))?;
    let kind = fields
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "text" {
        return Ok(None);
    }
    if !matches!(kind, "json_schema" | "json_object") {
        return Err(AdapterError::invalid_request().with_parameter("text.format"));
    }
    // A strict request must keep its native guarantee, not silently acquire an
    // instruction-based implementation merely because this option was enabled.
    if fields
        .get("strict")
        .is_some_and(|value| value != &Value::Bool(false))
    {
        return Err(AdapterError::parameter_unsupported_for(
            "text.format.strict",
        ));
    }
    if fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type" | "name" | "description" | "schema" | "strict"
        )
    }) || (kind == "json_object" && fields.len() != 1)
        || fields
            .get("description")
            .is_some_and(|value| !value.is_string())
    {
        return Err(AdapterError::invalid_request().with_parameter("text.format"));
    }
    let schema = if kind == "json_schema" {
        let name = fields
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(AdapterError::invalid_request().with_parameter("text.format.name"));
        }
        let schema = fields
            .get("schema")
            .filter(|schema| schema.is_object() || schema.is_boolean())
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("text.format.schema"))?;
        if schema.to_string().len() > MAX_SCHEMA_BYTES {
            return Err(AdapterError::invalid_request().with_parameter("text.format.schema"));
        }
        Some(
            validator(schema).map_err(|_| {
                AdapterError::invalid_request().with_parameter("text.format.schema")
            })?,
        )
    } else {
        None
    };
    Ok(Some(StructuredOutput {
        format: format.clone(),
        schema,
    }))
}

impl StructuredOutput {
    pub(super) fn instructions(&self) -> String {
        format!(
            "Return exactly one JSON value without fences or surrounding prose as the final answer. Relay validates the answer against the unchanged client format before release. Tool calls and refusals remain separate protocol items. This is not native constrained decoding. Required format: {}",
            self.format
        )
    }

    pub(super) fn validate(&self, response: &Value) -> Result<(), AdapterError> {
        let invalid =
            || AdapterError::upstream_response_invalid().with_parameter("output.structured_output");
        if response["status"] != "completed" {
            return Err(invalid());
        }
        let items = response["output"].as_array().ok_or_else(invalid)?;
        if items.iter().any(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        }) {
            return Ok(());
        }
        let mut text = String::new();
        let mut refusal = false;
        for item in items.iter().filter(|item| item["type"] == "message") {
            for part in item["content"].as_array().ok_or_else(invalid)? {
                match part["type"].as_str() {
                    Some("refusal") => refusal = true,
                    Some("output_text") => {
                        let fragment = part["text"].as_str().ok_or_else(invalid)?;
                        if text.len().saturating_add(fragment.len()) > MAX_ANSWER_BYTES {
                            return Err(invalid());
                        }
                        text.push_str(fragment);
                    }
                    _ => return Err(invalid()),
                }
            }
        }
        if refusal && text.is_empty() {
            return Ok(());
        }
        let answer: Value = serde_json::from_str(&text).map_err(|_| invalid())?;
        if self
            .schema
            .as_ref()
            .is_some_and(|schema| !schema.is_valid(&answer))
            || (self.schema.is_none() && !answer.is_object())
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validation_keeps_explicit_non_strict_and_never_claims_native_strict() {
        let mut request = json!({"text":{"format":{"type":"json_schema","name":"answer","strict":false,"schema":{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"],"additionalProperties":false}}}});
        assert!(prepare(&request).unwrap().is_some());
        request[OPTION] = json!(true);
        let format = prepare(&request).unwrap().unwrap();
        for (text, valid) in [
            ("{\"n\":3}", true),
            ("{\"n\":\"3\"}", false),
            ("{}", false),
            ("{\"n\":3} {}", false),
            ("```json\n{}\n```", false),
        ] {
            assert_eq!(format.validate(&json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":text}]}]})).is_ok(), valid);
        }
        request["text"]["format"]["strict"] = json!(true);
        assert_eq!(
            prepare(&request).err().unwrap().parameter(),
            Some("text.format.strict")
        );
        request["text"]["format"]["strict"] = json!(false);
        request["text"]["format"]["schema"] = json!({"$ref":"https://example.invalid/schema.json"});
        assert!(prepare(&request).is_err());
    }

    #[test]
    fn refusal_and_tool_turns_are_not_fabricated_into_json() {
        let format = prepare(&json!({OPTION:true,"text":{"format":{"type":"json_object"}}}))
            .unwrap()
            .unwrap();
        for item in [
            json!({"type":"function_call"}),
            json!({"type":"message","content":[{"type":"refusal","refusal":"No"}]}),
        ] {
            assert!(
                format
                    .validate(&json!({"status":"completed","output":[item]}))
                    .is_ok()
            );
        }
        assert!(
            format
                .validate(&json!({"status":"incomplete","output":[]}))
                .is_err()
        );
    }
}

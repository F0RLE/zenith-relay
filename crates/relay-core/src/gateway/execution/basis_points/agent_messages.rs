//! Encrypted client arguments must not escape BPS as falsely labelled plaintext.
use super::catalog::{ClientTool, client_tool_call_name, tool_spec};
use crate::protocol::{AdapterError, agent_messages::AgentMessageContext};
use serde_json::Value;

/// Only direct string properties currently have a portable encrypted-argument
/// contract. Reject deeper/ref/composite annotations rather than ignore them.
pub(super) fn validate_schema(schema: &Value) -> Result<(), AdapterError> {
    let Some(fields) = schema.as_object() else {
        return Ok(());
    };
    if !has_encrypted(schema) {
        return Ok(());
    }
    if fields.get("encrypted") == Some(&Value::Bool(true)) {
        return Err(unsupported());
    }
    // A reference can reuse an encrypted direct property at a second position.
    // Until reference-aware sealing exists, never emit that alias as plaintext.
    if has_reference(schema) {
        return Err(unsupported());
    }
    if let Some(properties) = fields.get("properties").and_then(Value::as_object) {
        for property in properties.values() {
            let encrypted = property.get("encrypted") == Some(&Value::Bool(true));
            let string_type = property.get("type").is_some_and(|kind| {
                kind == "string"
                    || kind.as_array().is_some_and(|types| {
                        types.contains(&Value::String("string".into()))
                            && types.iter().all(|kind| kind == "string" || kind == "null")
                    })
            });
            if encrypted && !string_type || has_encrypted_children(property) {
                return Err(unsupported());
            }
        }
    }
    // Root properties were checked above; every other schema position is
    // unsupported. Defaults/examples and literal property names are not schemas.
    let mut remaining = schema.clone();
    if let Some(fields) = remaining.as_object_mut() {
        fields.remove("properties");
    }
    if has_encrypted_children(&remaining) {
        return Err(unsupported());
    }
    Ok(())
}

fn unsupported() -> AdapterError {
    AdapterError::parameter_unsupported_for("tools.parameters.encrypted")
}

fn has_encrypted(schema: &Value) -> bool {
    schema.get("encrypted") == Some(&Value::Bool(true)) || has_encrypted_children(schema)
}

fn has_encrypted_children(schema: &Value) -> bool {
    children_match(schema, has_encrypted)
}

fn has_reference(schema: &Value) -> bool {
    ["$ref", "$dynamicRef", "$recursiveRef"]
        .iter()
        .any(|key| schema.get(key).is_some())
        || children_match(schema, has_reference)
}

fn children_match(schema: &Value, predicate: fn(&Value) -> bool) -> bool {
    for keyword in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
        "dependencies",
    ] {
        if schema
            .get(keyword)
            .and_then(Value::as_object)
            .is_some_and(|children| children.values().any(predicate))
        {
            return true;
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if schema
            .get(keyword)
            .and_then(Value::as_array)
            .is_some_and(|children| children.iter().any(predicate))
        {
            return true;
        }
    }
    [
        "items",
        "additionalItems",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
        "contentSchema",
    ]
    .into_iter()
    .any(|keyword| {
        schema.get(keyword).is_some_and(|child| {
            predicate(child)
                || child
                    .as_array()
                    .is_some_and(|children| children.iter().any(predicate))
        })
    })
}

pub(super) fn seal_output(
    response: &mut Value,
    tools: &[ClientTool],
    context: Option<&AgentMessageContext>,
) -> Result<(), AdapterError> {
    let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for item in output {
        if item["type"] != "function_call" {
            continue;
        }
        let Some(fields) = item.as_object() else {
            continue;
        };
        let name = client_tool_call_name(fields);
        let Some(tool) = tool_spec(tools, &name) else {
            continue;
        };
        let Some(properties) = super::schema::parameter_schema(tool)
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        if !properties
            .values()
            .any(|schema| schema.get("encrypted") == Some(&Value::Bool(true)))
        {
            continue;
        }
        let context = context.ok_or_else(unsupported)?;
        let mut arguments: Value =
            serde_json::from_str(item["arguments"].as_str().ok_or_else(unsupported)?)
                .map_err(|_| unsupported())?;
        for (name, schema) in properties {
            if schema.get("encrypted") != Some(&Value::Bool(true)) {
                continue;
            }
            if let Some(value) = arguments.get_mut(name).filter(|value| !value.is_null()) {
                let text = value.as_str().ok_or_else(unsupported)?;
                *value = context.seal_argument(&tool.key(), name, text)?.into();
            }
        }
        item["arguments"] = serde_json::to_string(&arguments)
            .map_err(|_| unsupported())?
            .into();
    }
    Ok(())
}

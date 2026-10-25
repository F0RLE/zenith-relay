//! Schema dialect conversion visits schema positions, never literal user data.
use super::{AdapterError, AdapterResult};
use serde_json::{json, Value};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
enum Direction {
    FromGemini,
    ToGemini,
}

pub(super) fn from_gemini(schema: &Value) -> AdapterResult<Value> {
    convert(schema, Direction::FromGemini)
}

pub(super) fn to_gemini(schema: &Value) -> AdapterResult<Value> {
    to_gemini_owned(schema.clone())
}

pub(super) fn to_gemini_owned(mut schema: Value) -> AdapterResult<Value> {
    transform(&mut schema, Direction::ToGemini)?;
    Ok(schema)
}

fn convert(schema: &Value, direction: Direction) -> AdapterResult<Value> {
    let mut result = schema.clone();
    transform(&mut result, direction)?;
    Ok(result)
}

fn transform(schema: &mut Value, direction: Direction) -> AdapterResult<()> {
    let Some(fields) = schema.as_object_mut() else {
        return Ok(());
    };
    let mut nullable = false;
    match direction {
        Direction::ToGemini => {
            // A harness annotation, not a JSON Schema constraint.
            fields.shift_remove("encrypted");
        }
        Direction::FromGemini => {
            if let Some(kind) = fields.get_mut("type") {
                let text = kind.as_str().ok_or_else(AdapterError::invalid_request)?;
                // Most large tool catalogs already use canonical types. Do not
                // allocate and replace every string on this no-op path.
                if !canonical_type(text) {
                    let normalized = text.to_ascii_lowercase();
                    if !canonical_type(&normalized) {
                        return Err(AdapterError::parameter_unsupported_for("schema.type"));
                    }
                    *kind = normalized.into();
                }
            }
            if let Some(value) = fields.shift_remove("nullable") {
                nullable = value.as_bool().ok_or_else(AdapterError::invalid_request)?;
            }
            for keyword in [
                "minItems",
                "maxItems",
                "minLength",
                "maxLength",
                "minProperties",
                "maxProperties",
            ] {
                if let Some(value) = fields.get_mut(keyword) {
                    if let Some(text) = value.as_str() {
                        *value = text
                            .parse::<u64>()
                            .map_err(|_| AdapterError::invalid_request())?
                            .into();
                    }
                }
            }
            if let Some(ordering) = fields.shift_remove("propertyOrdering") {
                if !ordering.as_array().is_some_and(Vec::is_empty) {
                    return Err(AdapterError::parameter_unsupported_for(
                        "schema.propertyOrdering",
                    ));
                }
            }
        }
    }
    for keyword in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
        "dependencies",
    ] {
        if let Some(children) = fields.get_mut(keyword).and_then(Value::as_object_mut) {
            for child in children.values_mut().filter(|child| child.is_object()) {
                transform(child, direction)?;
            }
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = fields.get_mut(keyword).and_then(Value::as_array_mut) {
            for child in children {
                transform(child, direction)?;
            }
        }
    }
    for keyword in [
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
    ] {
        if let Some(child) = fields.get_mut(keyword) {
            if let Some(children) = child.as_array_mut() {
                for child in children {
                    transform(child, direction)?;
                }
            } else {
                transform(child, direction)?;
            }
        }
    }
    // Wrap the complete non-null schema, not only its type: enum, anyOf and
    // other constraints must continue to admit the original nullable value.
    if nullable {
        let non_null = std::mem::take(schema);
        *schema = json!({"anyOf":[non_null,{"type":"null"}]});
    }
    Ok(())
}

fn canonical_type(kind: &str) -> bool {
    matches!(
        kind,
        "object" | "array" | "string" | "number" | "integer" | "boolean" | "null"
    )
}

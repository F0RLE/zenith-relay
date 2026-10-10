//! Tool schemas stay intact for validation. Only their prompt presentation is compacted.
use super::catalog::ClientTool;
use serde_json::{Map, Value};

pub(super) fn parameter_schema(tool: &ClientTool) -> Option<&Value> {
    ["parameters", "inputSchema", "input_schema"]
        .into_iter()
        .find_map(|key| tool.spec.get(key))
}

pub(super) fn validator(
    schema: &Value,
) -> Result<jsonschema::Validator, jsonschema::ValidationError<'_>> {
    // Keep schemas self-contained even if another dependency enables retrieval
    // through Cargo feature unification. Local $defs remain supported.
    jsonschema::options().offline().build(schema)
}

pub(super) fn matches(arguments: &Value, schema: &Value) -> bool {
    validator(schema).is_ok_and(|validator| validator.is_valid(arguments))
}

pub(super) fn prompt_schema(schema: &Value) -> String {
    compact(schema, 0).unwrap_or_else(|| schema.to_string())
}

fn compact(schema: &Value, depth: usize) -> Option<String> {
    if depth >= 8 {
        return None;
    }
    let object = schema.as_object()?;
    // Keep unfamiliar dialects, references, unions and constraints verbatim.
    // This whitelist is deliberately narrow: a compact rendering must not
    // quietly lose a keyword that changes the schema's meaning.
    let kind = object.get("type")?.as_str()?;
    let structural = match kind {
        "object" => &["type", "properties", "required", "additionalProperties"][..],
        "array" => &["type", "items"][..],
        "string" | "number" | "integer" | "boolean" | "null" => &["type"][..],
        _ => return None,
    };
    if object.keys().any(|key| {
        !structural.contains(&key.as_str())
            && !matches!(
                key.as_str(),
                "description"
                    | "title"
                    | "enum"
                    | "const"
                    | "default"
                    | "minimum"
                    | "maximum"
                    | "exclusiveMinimum"
                    | "exclusiveMaximum"
                    | "multipleOf"
                    | "minLength"
                    | "maxLength"
                    | "pattern"
                    | "minItems"
                    | "maxItems"
                    | "uniqueItems"
                    | "minProperties"
                    | "maxProperties"
            )
    }) {
        return None;
    }
    let shape = match kind {
        "object" => {
            let properties = object.get("properties")?.as_object()?;
            let required = object.get("required").and_then(Value::as_array);
            if required.is_some_and(|names| {
                names.iter().any(|name| {
                    name.as_str()
                        .is_none_or(|name| !properties.contains_key(name))
                })
            }) {
                return None;
            }
            let mut names = properties.keys().collect::<Vec<_>>();
            names.sort();
            let fields = names
                .into_iter()
                .map(|name| {
                    let optional = if required
                        .is_some_and(|names| names.iter().any(|value| value.as_str() == Some(name)))
                    {
                        ""
                    } else {
                        "?"
                    };
                    Some(format!(
                        "{}{optional}:{}",
                        Value::String(name.clone()),
                        compact(&properties[name], depth + 1)?
                    ))
                })
                .collect::<Option<Vec<_>>>()?;
            // Keep openness explicit, including the JSON Schema default.
            let additional = object
                .get("additionalProperties")
                .cloned()
                .unwrap_or(Value::Bool(true));
            if !additional.is_boolean() {
                return None;
            }
            format!("{{{}}} additionalProperties={additional}", fields.join(","))
        }
        "array" => format!("[{}]", compact(object.get("items")?, depth + 1)?),
        _ => kind.to_string(),
    };
    let metadata = object
        .iter()
        .filter(|(key, _)| !structural.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    Some(if metadata.is_empty() {
        shape
    } else {
        format!("{shape} {metadata}", metadata = Value::Object(metadata))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_schema_keeps_constraints_and_reduces_repetition() {
        let schema = json!({
            "type":"object", "properties":{
                "query":{"type":"string","description":"Search text","minLength":1},
                "limit":{"type":"integer","minimum":1,"maximum":100},
                "tags":{"type":"array","items":{"type":"string"},"uniqueItems":true}
            }, "required":["query"],"additionalProperties":false
        });
        let prompt = prompt_schema(&schema);
        assert!(prompt.len() < schema.to_string().len());
        assert!(prompt.contains("\"query\":string"));
        assert!(prompt.contains("\"limit\"?:integer"));
        assert!(prompt.contains("additionalProperties=false"));
        assert!(prompt.contains("\"minLength\":1"));
        assert!(matches(&json!({"query":"x","limit":2}), &schema));
        assert!(!matches(&json!({"query":"x","limit":2.5}), &schema));
        assert!(!matches(&json!({"query":""}), &schema));
    }

    #[test]
    fn complex_schemas_stay_verbatim_and_validate_the_original() {
        let schema = json!({
            "$defs":{"name":{"type":"string","minLength":2}},
            "type":"object","properties":{"name":{"$ref":"#/$defs/name"}},
            "required":["name"],"additionalProperties":false
        });
        assert_eq!(prompt_schema(&schema), schema.to_string());
        assert!(matches(&json!({"name":"ok"}), &schema));
        assert!(!matches(&json!({"name":"x"}), &schema));
        assert!(validator(&json!({"$ref":"https://invalid.example/schema"})).is_err());
        assert!(validator(&json!({"$ref":"file:///synthetic-schema.json"})).is_err());
    }
}

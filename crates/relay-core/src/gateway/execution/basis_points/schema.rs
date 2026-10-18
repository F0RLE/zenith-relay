//! Compact prompt presentation; argument validation always uses the original schema.
use super::catalog::ClientTool;
use serde_json::Value;

pub(super) const PROMPT_SCHEMA_NOTATION: &str = concat!(
    "Parameter notation: object{\"key\":type,\"optional\"?:type} marks required and optional keys; ",
    "[type] is an array; ... allows extra keys and ...:type constrains them. ",
    "Without ... extra keys are forbidden. @ followed by JSON preserves descriptions and constraints. ",
    "Unfamiliar or complex schemas remain complete JSON Schema. Obey every constraint. ",
    "This notation describes arguments, not a different format for tool calls. ",
);

const MAX_PROMPT_SCHEMA_DEPTH: usize = 16;

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
    let original = schema.to_string();
    compact_schema(schema, 0)
        .filter(|compact| compact.len() < original.len())
        .unwrap_or(original)
}

fn compact_schema(schema: &Value, depth: usize) -> Option<String> {
    let object = schema.as_object()?;
    if depth >= MAX_PROMPT_SCHEMA_DEPTH {
        return None;
    }
    // Only consume well-understood scaffolding. References, dialect changes,
    // unions, dependent/conditional schemas and unknown extensions stay JSON.
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "title"
                | "description"
                | "default"
                | "examples"
                | "deprecated"
                | "readOnly"
                | "writeOnly"
                | "enum"
                | "const"
                | "format"
                | "pattern"
                | "minLength"
                | "maxLength"
                | "minimum"
                | "maximum"
                | "exclusiveMinimum"
                | "exclusiveMaximum"
                | "multipleOf"
                | "minItems"
                | "maxItems"
                | "uniqueItems"
                | "minProperties"
                | "maxProperties"
        )
    }) {
        return None;
    }
    let mut annotations = object.clone();
    annotations.remove("type");
    let child =
        |value: &Value| compact_schema(value, depth + 1).unwrap_or_else(|| value.to_string());
    let mut rendered = match object.get("type")?.as_str()? {
        "object" => {
            let properties = match object.get("properties") {
                Some(value) => Some(value.as_object()?),
                None => None,
            };
            let required = match object.get("required") {
                Some(value) => Some(value.as_array()?),
                None => None,
            };
            let mut names = properties
                .map(|properties| properties.keys().collect::<Vec<_>>())
                .unwrap_or_default();
            names.sort();
            if required.is_some_and(|required| {
                required.iter().any(|name| {
                    name.as_str()
                        .is_none_or(|name| !properties.is_some_and(|p| p.contains_key(name)))
                })
            }) {
                return None;
            }
            let mut fields = names
                .into_iter()
                .map(|name| {
                    let optional = if required
                        .is_some_and(|required| required.iter().any(|v| v.as_str() == Some(name)))
                    {
                        ""
                    } else {
                        "?"
                    };
                    format!(
                        "{}{optional}:{}",
                        Value::String(name.clone()),
                        child(&properties.unwrap()[name])
                    )
                })
                .collect::<Vec<_>>();
            match object.get("additionalProperties") {
                None | Some(Value::Bool(true)) => fields.push("...".to_string()),
                Some(Value::Bool(false)) => {}
                Some(value @ Value::Object(_)) => fields.push(format!("...:{}", child(value))),
                _ => return None,
            }
            for key in ["properties", "required", "additionalProperties"] {
                annotations.remove(key);
            }
            format!("object{{{}}}", fields.join(","))
        }
        "array" => {
            let items = object.get("items")?;
            if !items.is_object() && !items.is_boolean() {
                return None;
            }
            annotations.remove("items");
            format!("[{}]", child(items))
        }
        kind @ ("string" | "integer" | "number" | "boolean" | "null") => kind.to_string(),
        _ => return None,
    };
    if !annotations.is_empty() {
        rendered.push_str(" @");
        rendered.push_str(&Value::Object(annotations).to_string());
    }
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_presentation_preserves_descriptions_constraints_and_validation() {
        let schema = json!({
            "type":"object", "properties":{
                "query":{"type":"string","description":"Search text","minLength":1},
                "limit":{"type":"integer","minimum":1,"maximum":100},
                "tags":{"type":"array","items":{"type":"string"},"uniqueItems":true}
            }, "required":["query"],"additionalProperties":false
        });
        let prompt = prompt_schema(&schema);
        assert!(prompt.starts_with("object{"));
        assert!(prompt.contains("\"query\":string @"));
        assert!(prompt.contains("\"limit\"?:integer @"));
        assert!(prompt.contains("\"description\":\"Search text\""));
        assert!(prompt.contains("\"uniqueItems\":true"));
        assert!(!prompt.contains("..."));
        assert!(prompt.len() < schema.to_string().len());
        assert!(matches(&json!({"query":"x","limit":2}), &schema));
        assert!(!matches(&json!({"query":"x","limit":2.5}), &schema));
        assert!(!matches(&json!({"query":""}), &schema));
    }

    #[test]
    fn unsupported_keywords_and_dialects_keep_complete_json() {
        for schema in [
            json!({"type":"object", "oneOf":[{"required":["x"]}]}),
            json!({"type":"object", "$schema":"https://json-schema.org/draft/2020-12/schema"}),
            json!({"type":"object", "unevaluatedProperties":false}),
            json!({"type":"array", "items":{}, "contains":{"const":1}}),
            json!({"type":"string", "x-private-contract":{"version":1}}),
            json!({"type":["string","null"]}),
            json!(false),
        ] {
            assert_eq!(prompt_schema(&schema), schema.to_string());
        }
    }

    #[test]
    fn compact_schema_does_not_mutate_contract_and_preserves_extra_keys() {
        let schema = json!({"type":"object","properties":{
            "path?\"":{"type":"string","enum":["a","b"],"default":"a"}
        },"additionalProperties":{"type":"integer","minimum":1}});
        let before = schema.clone();
        let prompt = prompt_schema(&schema);
        assert!(prompt.contains("...:integer @"));
        assert!(prompt.contains("\"path?\\\"\"?:string"));
        assert_eq!(schema, before);
        assert!(matches(&json!({"extra":2}), &schema));
        assert!(!matches(&json!({"extra":0}), &schema));
        assert!(!matches(&json!({"path?\"":"c"}), &schema));
        assert!(prompt_schema(&json!({"type":"object"})).contains("..."));
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

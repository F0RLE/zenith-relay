//! Tool schemas use the same lossless JSON representation for prompts and validation.
use super::catalog::ClientTool;
use serde_json::Value;

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
    // Do not teach a private schema language while validating against JSON Schema.
    schema.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prompt_schema_is_lossless_json_including_descriptions_and_constraints() {
        let schema = json!({
            "type":"object", "properties":{
                "query":{"type":"string","description":"Search text","minLength":1},
                "limit":{"type":"integer","minimum":1,"maximum":100},
                "tags":{"type":"array","items":{"type":"string"},"uniqueItems":true}
            }, "required":["query"],"additionalProperties":false
        });
        let prompt = prompt_schema(&schema);
        assert_eq!(serde_json::from_str::<Value>(&prompt).unwrap(), schema);
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

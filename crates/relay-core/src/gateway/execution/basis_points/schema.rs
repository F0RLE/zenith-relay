//! Compact prompt presentation; argument validation always uses the original schema.
use super::catalog::ClientTool;
use crate::protocol::AdapterError;
use serde_json::Value;

mod validation;
pub(super) use validation::{cache_memory_diagnostics, release_expired_cache_entries, validator};

pub(super) const PROMPT_SCHEMA_NOTATION: &str = concat!(
    "Parameter notation: object{\"key\":type,\"optional\"?:type} marks required and optional keys; ",
    "[type] is an array; ... allows extra keys and ...:type constrains them. ",
    "Without ... extra keys are forbidden. @ followed by JSON preserves descriptions and constraints. ",
    "Closed objects retain an explicit additionalProperties:false constraint. ",
    "Unfamiliar or complex schemas remain complete JSON Schema. Obey every constraint. ",
    "This notation describes arguments, not a different format for tool calls. ",
);

const MAX_PROMPT_SCHEMA_DEPTH: usize = 16;

pub(super) fn parameter_schema(tool: &ClientTool) -> Option<&Value> {
    ["parameters", "inputSchema", "input_schema"]
        .into_iter()
        .find_map(|key| tool.spec.get(key))
}

pub(super) fn matches(arguments: &Value, schema: &Value) -> bool {
    validator(schema).is_ok_and(|validator| validator.is_valid(arguments))
}

pub(super) fn validate(arguments: &Value, schema: &Value) -> Result<(), AdapterError> {
    let validator = validator(schema).map_err(|error| validation_error(&error, true))?;
    validator
        .validate(arguments)
        .map_err(|error| validation_error(&error, false))
}

pub(super) fn validation_error(
    error: &jsonschema::ValidationError<'_>,
    compiling: bool,
) -> AdapterError {
    let path = error.schema_path().as_str();
    // Schema property names, reference URLs and instance values are private.
    let keyword = match path.rsplit('/').next().unwrap_or_default() {
        "type" => "type",
        "required" => "required",
        "additionalProperties" => "additionalProperties",
        "enum" => "enum",
        "const" => "const",
        "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => "number",
        "minLength" | "maxLength" | "pattern" | "format" => "string",
        "minItems" | "maxItems" | "uniqueItems" | "contains" => "array",
        "minProperties" | "maxProperties" | "propertyNames" => "object",
        "anyOf" | "allOf" | "oneOf" | "not" => "composition",
        "$ref" | "$dynamicRef" => "reference",
        _ => "other",
    };
    let (error_result, phase) = if compiling {
        (AdapterError::tool_schema_invalid(), "compile")
    } else {
        (AdapterError::upstream_tool_arguments_invalid(), "arguments")
    };
    error_result.with_schema_diagnostic(
        phase,
        keyword,
        path.bytes().filter(|byte| *byte == b'/').count(),
        error
            .instance_path()
            .as_str()
            .bytes()
            .filter(|byte| *byte == b'/')
            .count(),
    )
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
    let kind = object.get("type")?.as_str()?;
    let child =
        |value: &Value| compact_schema(value, depth + 1).unwrap_or_else(|| value.to_string());
    let mut rendered = match kind {
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
            format!("object{{{}}}", fields.join(","))
        }
        "array" => {
            let items = object.get("items")?;
            if !items.is_object() && !items.is_boolean() {
                return None;
            }
            format!("[{}]", child(items))
        }
        kind @ ("string" | "integer" | "number" | "boolean" | "null") => kind.to_string(),
        _ => return None,
    };
    // Borrow annotations in stable order instead of cloning each nested schema.
    let annotations: std::collections::BTreeMap<_, _> = object
        .iter()
        .filter(|(key, value)| match key.as_str() {
            "type" => false,
            "properties" | "required" if kind == "object" => false,
            "additionalProperties" if kind == "object" => value.as_bool() == Some(false),
            "items" if kind == "array" => false,
            _ => true,
        })
        .collect();
    if !annotations.is_empty() {
        rendered.push_str(" @");
        rendered.push_str(
            &serde_json::to_string(&annotations).expect("JSON schema annotations are serializable"),
        );
    }
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    mod benchmark;

    #[test]
    fn schema_diagnostics_separate_compilation_and_arguments_without_private_data() {
        let schema = json!({"type":"object","properties":{
            "synthetic-private-key":{"type":"integer"}
        }});
        let error = validate(
            &json!({"synthetic-private-key":"synthetic-private-value"}),
            &schema,
        )
        .unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.args.schema"));
        assert!(error.to_string().contains("schema_phase=arguments"));
        assert!(error.to_string().contains("schema_keyword=type"));
        assert!(
            error
                .to_string()
                .contains("schema_depth=3, instance_depth=1")
        );
        let invalid = validate(
            &json!({}),
            &json!({"$ref":"https://synthetic-private.invalid/schema"}),
        )
        .unwrap_err();
        assert_eq!(invalid.parameter(), Some("tools.parameters"));
        assert!(invalid.to_string().contains("schema_phase=compile"));
        for error in [error, invalid] {
            assert!(!error.to_string().contains("synthetic-private"));
            assert!(!format!("{error:?}").contains("synthetic-private"));
        }
    }

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
        assert!(prompt.contains("\"additionalProperties\":false"));
        assert!(prompt.len() < schema.to_string().len());
        assert!(matches(&json!({"query":"x","limit":2}), &schema));
        assert!(!matches(&json!({"query":"x","limit":2.5}), &schema));
        assert!(!matches(&json!({"query":""}), &schema));
    }

    #[test]
    fn compact_presentation_is_stable_when_schema_keywords_are_reordered() {
        let first = json!({
            "type":"object","properties":{
                "limit":{"type":"integer","description":"Synthetic limit","minimum":1,"maximum":10},
                "enabled":{"type":"boolean","default":false}
            },"required":["limit"],"description":"Synthetic tool","additionalProperties":false
        });
        let reordered = json!({
            "description":"Synthetic tool","additionalProperties":false,"required":["limit"],
            "properties":{
                "enabled":{"default":false,"type":"boolean"},
                "limit":{"description":"Synthetic limit","maximum":10,"minimum":1,"type":"integer"}
            },"type":"object"
        });
        assert_ne!(first.to_string(), reordered.to_string());
        assert!(prompt_schema(&first).starts_with("object{"));
        assert_eq!(prompt_schema(&first), prompt_schema(&reordered));
        for arguments in [
            json!({"limit":1}),
            json!({"limit":0}),
            json!({"limit":2,"extra":true}),
        ] {
            assert_eq!(matches(&arguments, &first), matches(&arguments, &reordered));
        }
    }

    #[test]
    fn closed_objects_remain_explicit_at_every_compact_schema_depth() {
        let schema = json!({
            "type":"object","properties":{
                "path":{"type":"string","description":"Synthetic file path"},
                "options":{"type":"object","properties":{
                    "enabled":{"type":"boolean"},"limit":{"type":"integer"}
                },"required":["enabled"],"additionalProperties":false}
            },"required":["path","options"],"additionalProperties":false
        });
        let before = schema.clone();
        let prompt = prompt_schema(&schema);
        assert!(prompt.starts_with("object{"));
        assert_eq!(prompt.matches("\"additionalProperties\":false").count(), 2);
        assert!(prompt.len() < schema.to_string().len());
        assert_eq!(schema, before);
        let valid = json!({"path":"synthetic.txt","options":{"enabled":true}});
        assert!(matches(&valid, &schema));
        for extra_at_root in [true, false] {
            let mut invalid = valid.clone();
            let target = if extra_at_root {
                &mut invalid
            } else {
                &mut invalid["options"]
            };
            target["synthetic-private-extra"] = json!("synthetic-private-value");
            let error = validate(&invalid, &schema).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("schema_keyword=additionalProperties")
            );
            assert!(!format!("{error} {error:?}").contains("synthetic-private"));
        }
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

use super::*;

fn tool_with_schema(schema: Value) -> ClientTool {
    let request = json!({"tools":[{
        "type":"function","name":"synthetic_presentation","parameters":schema
    }]});
    let mut tool = client_tools(&request).unwrap().remove(0);
    tool.presentation = Arc::default();
    tool.schema_size = Arc::default();
    tool
}

#[test]
fn cold_presentation_preserves_schema_text_and_byte_accounting() {
    for schema in [
        json!({"type":"object","properties":{
            "path":{"type":"string","description":"Synthetic path","minLength":1},
            "count":{"type":"integer","minimum":1,"default":2}
        },"required":["path"],"additionalProperties":false}),
        json!({"type":"object","properties":{
            "options":{"type":"object","properties":{"enabled":{"type":"boolean"}},
                "required":["enabled"],"additionalProperties":false}
        },"required":["options"],"additionalProperties":false}),
        json!({"$defs":{"value":{"type":"string"}},"$ref":"#/$defs/value"}),
        json!({"type":["string","null"],"description":"Synthetic union"}),
        json!({"type":"string","x-synthetic-extension":{"preserve":"verbatim"}}),
        json!(false),
    ] {
        let original = schema.to_string();
        let expected_compact = super::super::super::schema::prompt_schema(&schema);
        for full_first in [false, true] {
            let tool = tool_with_schema(schema.clone());
            let prefix = "- synthetic_presentation (function). Parameters: ";
            let _ = tool.description(full_first);
            assert_eq!(tool.description(true), format!("{prefix}{original}"));
            assert_eq!(
                tool.description(false),
                format!("{prefix}{expected_compact}")
            );
            assert_eq!(tool.schema_bytes(true), original.len() as u64);
            assert_eq!(tool.schema_bytes(false), expected_compact.len() as u64);
            assert_eq!(tool.spec["parameters"], schema);
        }
    }
}

#[test]
#[ignore = "manual synthetic cold presentation benchmark; no provider or token measurements"]
fn measure_cold_catalog_presentation() {
    use std::{hint::black_box, time::Instant};

    for complex in [false, true] {
        let mut schema = json!({
            "type":"object","properties":(0..64).map(|index| (
                format!("field_{index}"),
                json!({"type":"string","description":"Synthetic parameter. ".repeat(8),
                    "minLength":1,"maxLength":4096})
            )).collect::<serde_json::Map<_, _>>(),
            "required":["field_0"],"additionalProperties":false
        });
        if complex {
            schema["allOf"] = json!([{"required":["field_1"]}]);
        }
        let template = tool_with_schema(schema);
        let mut samples = Vec::new();
        for _ in 0..9 {
            let started = Instant::now();
            for _ in 0..100 {
                let mut tool = template.clone();
                tool.presentation = Arc::default();
                black_box(tool.description(false));
                black_box(tool.description(true));
            }
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        eprintln!(
            "cold-catalog complex={complex} schema_bytes={} compact_bytes={} iterations=100 median_us={}",
            template.schema_bytes(true),
            template.schema_bytes(false),
            samples[4].as_micros()
        );
    }
}

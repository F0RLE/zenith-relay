use super::*;

#[test]
fn wide_required_properties_preserve_exact_compact_presentation() {
    for width in [0, 1, 8, 64, 256] {
        for required_count in [0, width / 2, width] {
            let schema = json!({
                "type":"object",
                "properties":(0..width).rev().map(|index| (
                    format!("field_{index:03}"), json!({"type":"string"})
                )).collect::<serde_json::Map<_, _>>(),
                "required":(0..required_count).rev()
                    .map(|index| format!("field_{index:03}")).collect::<Vec<_>>(),
                "additionalProperties":false
            });
            let before = schema.clone();
            let fields = (0..width)
                .map(|index| {
                    let optional = if index < required_count { "" } else { "?" };
                    format!("\"field_{index:03}\"{optional}:string")
                })
                .collect::<Vec<_>>()
                .join(",");
            let expected = format!("object{{{fields}}} @{{\"additionalProperties\":false}}");
            assert_eq!(
                compact_schema(&schema, 0).as_deref(),
                Some(expected.as_str())
            );
            assert_eq!(schema, before);
        }
    }
}

#[test]
fn required_order_and_duplicate_names_do_not_change_presentation() {
    let mut schema = json!({
        "type":"object","properties":{
            "z":{"type":"string"},"a\"\\\n":{"type":"integer"},
            "\u{03b2}":{"type":"boolean"},"Z":{"type":"number"}
        },"required":["\u{03b2}","a\"\\\n"],"additionalProperties":false
    });
    let expected = "object{\"Z\"?:number,\"a\\\"\\\\\\n\":integer,\"z\"?:string,\"\u{03b2}\":boolean} @{\"additionalProperties\":false}";
    assert_eq!(compact_schema(&schema, 0).as_deref(), Some(expected));
    schema["required"] = json!(["a\"\\\n", "\u{03b2}", "a\"\\\n"]);
    assert_eq!(compact_schema(&schema, 0).as_deref(), Some(expected));
}

#[test]
fn invalid_required_declarations_keep_the_original_json() {
    for required in [
        json!(null),
        json!("field"),
        json!({"field":true}),
        json!([1]),
        json!(["field", false]),
        json!(["missing"]),
    ] {
        let schema = json!({
            "type":"object","properties":{"field":{"type":"string"}},
            "required":required,"additionalProperties":false
        });
        assert!(compact_schema(&schema, 0).is_none());
        assert_eq!(prompt_schema(&schema), schema.to_string());
    }
}

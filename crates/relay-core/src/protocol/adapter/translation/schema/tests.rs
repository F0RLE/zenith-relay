use super::*;

#[test]
fn gemini_openapi_nullable_and_bounds_keep_validation_semantics() {
    let literal = json!({"type":"OBJECT","nullable":true,"encrypted":"literal data"});
    let schema = from_gemini(&json!({
        "type":"OBJECT",
        "properties":{
            "items":{"type":"ARRAY","nullable":true,"maxItems":"2","items":{"type":"STRING","enum":["OBJECT"]}},
            "literal":{"type":"OBJECT","default":literal}
        },
        "required":["items"]
    })).unwrap();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["literal"]["default"], literal);
    let validator = jsonschema::validator_for(&schema).unwrap();
    for value in [json!({"items":null}), json!({"items":["OBJECT"]})] {
        assert!(validator.is_valid(&value), "{value}");
    }
    for value in [
        json!({}),
        json!({"items":["other"]}),
        json!({"items":["OBJECT","OBJECT","OBJECT"]}),
    ] {
        assert!(!validator.is_valid(&value), "{value}");
    }
}

#[test]
fn schema_annotations_never_rewrite_property_names_or_literals() {
    let literal = json!({"encrypted":true,"type":"OBJECT","nullable":true});
    let schema = json!({"type":"object","encrypted":true,"properties":{
        "encrypted":{"type":"object","default":literal,"encrypted":true},
        "type":{"type":"string","enum":["OBJECT"]}
    }});
    let converted = to_gemini(&schema).unwrap();
    assert!(converted.get("encrypted").is_none());
    assert!(converted["properties"]["encrypted"]
        .get("encrypted")
        .is_none());
    assert_eq!(converted["properties"]["encrypted"]["default"], literal);
    assert_eq!(converted["properties"]["type"]["enum"], json!(["OBJECT"]));
    assert_eq!(schema["encrypted"], true);
}

#[test]
fn unrepresentable_openapi_controls_are_not_silently_discarded() {
    for schema in [
        json!({"type":"TYPE_UNSPECIFIED"}),
        json!({"type":"OBJECT","propertyOrdering":["answer"]}),
    ] {
        assert_eq!(
            from_gemini(&schema).unwrap_err().code(),
            "adapter_parameter_unsupported"
        );
    }
    for schema in [
        json!({"type":"ARRAY","maxItems":"not-an-integer"}),
        json!({"type":"STRING","nullable":"true"}),
    ] {
        assert_eq!(
            from_gemini(&schema).unwrap_err().code(),
            "adapter_invalid_request"
        );
    }
}

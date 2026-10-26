use super::*;

#[test]
#[ignore = "synthetic local CPU probe, not provider latency"]
fn measure_canonical_gemini_schema_conversion() {
    let schema = json!({"type":"object","properties":(0..128).map(|index| {
        (format!("field_{index}"), json!({"type":"string","description":"synthetic annotation","maxLength":256}))
    }).collect::<serde_json::Map<String, Value>>(),"additionalProperties":false});
    for _ in 0..20 {
        std::hint::black_box(from_gemini(&schema).unwrap());
    }
    let mut trials = Vec::new();
    for _ in 0..9 {
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(from_gemini(std::hint::black_box(&schema)).unwrap());
        }
        trials.push(started.elapsed().as_nanos() / 1000);
    }
    trials.sort_unstable();
    println!(
        "canonical_gemini_schema ns/op min={} median={} max={}",
        trials[0], trials[4], trials[8]
    );
    assert_eq!(
        to_gemini(&schema.clone()).unwrap(),
        to_gemini_owned(schema.clone()).unwrap()
    );
    let mut borrowed = Vec::new();
    let mut owned = Vec::new();
    for trial in 0..9 {
        for use_owned in if trial % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = std::time::Instant::now();
            for _ in 0..1000 {
                let result = if use_owned {
                    to_gemini_owned(schema.clone())
                } else {
                    to_gemini(&schema.clone())
                };
                std::hint::black_box(result.unwrap());
            }
            (if use_owned { &mut owned } else { &mut borrowed })
                .push(started.elapsed().as_nanos() / 1000);
        }
    }
    borrowed.sort_unstable();
    owned.sort_unstable();
    println!(
        "owned_gemini_schema ns/op borrowed_of_owned={} owned={}",
        borrowed[4], owned[4]
    );
}

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
    assert!(
        converted["properties"]["encrypted"]
            .get("encrypted")
            .is_none()
    );
    assert_eq!(converted["properties"]["encrypted"]["default"], literal);
    assert_eq!(converted["properties"]["type"]["enum"], json!(["OBJECT"]));
    assert_eq!(schema["encrypted"], true);
}

#[test]
fn canonical_schema_fast_path_keeps_order_literals_and_exact_numbers() {
    let schema: Value = serde_json::from_str(r#"{"type":"object","properties":{"z":{"type":"integer","maximum":9007199254740993},"a":{"type":"string","default":"OBJECT"}},"required":["z"]}"#).unwrap();
    assert_eq!(
        serde_json::to_vec(&from_gemini(&schema).unwrap()).unwrap(),
        serde_json::to_vec(&schema).unwrap()
    );
    let upper = json!({"type":"OBJECT","properties":{"x":{"type":"STRING"}}});
    assert_eq!(
        from_gemini(&upper).unwrap(),
        json!({"type":"object","properties":{"x":{"type":"string"}}})
    );
    let annotated = json!({"type":"object","encrypted":true,"properties":{"x":{"type":"string","default":{"encrypted":true}}}});
    assert_eq!(
        to_gemini_owned(annotated.clone()).unwrap(),
        to_gemini(&annotated).unwrap()
    );
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

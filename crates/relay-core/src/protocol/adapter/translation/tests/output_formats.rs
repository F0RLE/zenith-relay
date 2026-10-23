use super::*;

fn with_format(client: WireApi, format: Value) -> Value {
    let mut request = input(client);
    match client {
        WireApi::Responses => request["text"] = json!({"format":format}),
        WireApi::ChatCompletions => request["response_format"] = format,
        WireApi::Messages => request["output_config"] = json!({"format":format}),
        WireApi::Gemini => unreachable!(),
    }
    request
}

#[test]
fn converted_output_formats_reject_constraints_in_simple_modes() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        for mode in ["text", "json_object"] {
            for (field, constraint) in [
                ("schema", json!({"type":"object"})),
                ("json_schema", json!({"schema":{"type":"object"}})),
                ("strict", json!(true)),
                ("strict", json!(false)),
                ("name", json!("answer")),
                ("description", json!("A constrained answer")),
            ] {
                let mut format = json!({"type":mode});
                format[field] = constraint;
                let request = with_format(client, format);
                assert!(
                    decode::request(client, &request).is_err(),
                    "{client:?}: {request}"
                );
                assert!(prepare(client, client, &request, false).is_passthrough());
            }
        }
    }
}

#[test]
fn converted_output_formats_reject_mixed_schema_envelopes() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        for (field, constraint) in [
            ("schema", json!({"type":"array"})),
            ("strict", json!(true)),
            ("name", json!("other")),
            ("description", json!("Outer constraint")),
        ] {
            let mut format = json!({"type":"json_schema","json_schema":{
                "name":"answer","schema":{"type":"object"}
            }});
            format[field] = constraint;
            let request = with_format(client, format);
            assert!(
                decode::request(client, &request).is_err(),
                "{client:?}: {request}"
            );
        }
        let request = with_format(
            client,
            json!({"type":"json_schema","json_schema":{
                "type":"json_object","schema":{"type":"object"}
            }}),
        );
        assert!(decode::request(client, &request).is_err());
    }
}

#[test]
fn converted_output_formats_reject_invalid_schema_names() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        for name in [json!(false), json!(1), json!([]), json!({}), json!("")] {
            for nested in [false, true] {
                let fields = json!({"name":name,"schema":{"type":"object"}});
                let format = if nested {
                    json!({"type":"json_schema","json_schema":fields})
                } else {
                    let mut format = fields;
                    format["type"] = "json_schema".into();
                    format
                };
                let request = with_format(client, format);
                assert!(
                    decode::request(client, &request).is_err(),
                    "{client:?}: {request}"
                );
            }
        }
    }
}

#[test]
fn converted_output_formats_keep_default_names_and_nullable_controls() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        for fields in [
            json!({}),
            json!({"name":null,"description":null,"strict":null}),
        ] {
            for nested in [false, true] {
                if nested && client == WireApi::Responses {
                    continue;
                }
                let mut format = fields.clone();
                format["schema"] = json!({"type":"object"});
                let format = if nested {
                    json!({"type":"json_schema","json_schema":format})
                } else {
                    format["type"] = "json_schema".into();
                    format
                };
                let request = with_format(client, format);
                for upstream in [WireApi::Responses, WireApi::ChatCompletions] {
                    if upstream == client {
                        continue;
                    }
                    for stream in [false, true] {
                        let prepared = prepare(client, upstream, &request, stream);
                        let path = if upstream == WireApi::Responses {
                            "/text/format/name"
                        } else {
                            "/response_format/json_schema/name"
                        };
                        assert_eq!(
                            prepared.upstream_body().pointer(path),
                            Some(&json!("response"))
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn converted_gemini_output_formats_reject_invalid_mime_types() {
    for mime in [json!(false), json!(1), json!([]), json!({})] {
        let mut request = input(WireApi::Gemini);
        request["generationConfig"] = json!({"responseMimeType":mime});
        assert_eq!(
            decode::request(WireApi::Gemini, &request)
                .unwrap_err()
                .parameter(),
            Some("generationConfig.responseMimeType")
        );
        assert!(prepare(WireApi::Gemini, WireApi::Gemini, &request, false).is_passthrough());
    }
    let mut request = input(WireApi::Gemini);
    request["generationConfig"] = json!({"responseMimeType":null});
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        let prepared = prepare(WireApi::Gemini, upstream, &request, false);
        assert!(decode::request(upstream, prepared.upstream_body())
            .unwrap()
            .output_format
            .is_none());
    }
}

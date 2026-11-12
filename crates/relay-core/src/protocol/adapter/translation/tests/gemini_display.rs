use super::*;

mod roundtrip;

const PARAMETER: &str = "generationConfig.thinkingConfig.includeThoughts";

fn request(include: Value) -> Value {
    let mut request = input(WireApi::Gemini);
    request["generationConfig"] =
        json!({"maxOutputTokens":4096,"thinkingConfig":{"includeThoughts":include}});
    request
}

#[test]
fn gemini_display_only_omission_preserves_provider_defaults_and_native_fields() {
    let hidden = request(json!(false));
    let mut baseline = hidden.clone();
    baseline["generationConfig"]["thinkingConfig"]
        .as_object_mut()
        .unwrap()
        .remove("includeThoughts");
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        for stream in [false, true] {
            let expected = prepare(WireApi::Gemini, upstream, &baseline, stream);
            let omitted = prepare(WireApi::Gemini, upstream, &hidden, stream);
            assert_eq!(omitted.upstream_body(), expected.upstream_body());
        }
    }
    assert_eq!(
        prepare(WireApi::Gemini, WireApi::Gemini, &hidden, false).upstream_body(),
        &hidden
    );
}

#[test]
fn gemini_display_only_summary_never_guesses_an_effort_or_messages_thinking_mode() {
    let visible = request(json!(true));
    let responses = prepare(WireApi::Gemini, WireApi::Responses, &visible, false);
    assert_eq!(
        responses.upstream_body()["reasoning"],
        json!({"summary":"auto"})
    );
    for upstream in [WireApi::ChatCompletions, WireApi::Messages] {
        let error = try_convert(WireApi::Gemini, upstream, &visible, false).unwrap_err();
        assert_eq!(error.parameter(), Some(PARAMETER));
    }
    assert_eq!(
        prepare(WireApi::Gemini, WireApi::Gemini, &visible, false).upstream_body(),
        &visible
    );
}

#[test]
fn gemini_display_preserves_explicit_effort_and_exact_messages_budget() {
    for include in [false, true] {
        let mut effort = request(json!(include));
        effort["generationConfig"]["thinkingConfig"]["thinkingLevel"] = json!("high");
        let responses = prepare(WireApi::Gemini, WireApi::Responses, &effort, false);
        assert_eq!(responses.upstream_body()["reasoning"]["effort"], "high");
        assert_eq!(
            responses.upstream_body()["reasoning"]["summary"],
            if include { json!("auto") } else { Value::Null }
        );
        let messages = prepare(WireApi::Gemini, WireApi::Messages, &effort, false);
        assert_eq!(
            messages.upstream_body()["thinking"],
            json!({"type":"adaptive","display":if include { "summarized" } else { "omitted" }})
        );
        assert_eq!(messages.upstream_body()["output_config"]["effort"], "high");
        if !include {
            let chat = prepare(WireApi::Gemini, WireApi::ChatCompletions, &effort, false);
            assert_eq!(chat.upstream_body()["reasoning_effort"], "high");
        } else {
            let error =
                try_convert(WireApi::Gemini, WireApi::ChatCompletions, &effort, false).unwrap_err();
            assert_eq!(error.parameter(), Some(PARAMETER));
        }
        let mut budget = request(json!(include));
        budget["generationConfig"]["thinkingConfig"]["thinkingBudget"] = json!(2048);
        let messages = prepare(WireApi::Gemini, WireApi::Messages, &budget, false);
        assert_eq!(
            messages.upstream_body()["thinking"],
            json!({"type":"enabled","budget_tokens":2048,"display":if include { "summarized" } else { "omitted" }})
        );
        assert_eq!(messages.upstream_body()["max_tokens"], 4096);
    }
}

#[test]
fn gemini_display_does_not_enable_zero_budget_or_known_non_reasoning_models() {
    for include in [false, true] {
        let mut disabled = request(json!(include));
        disabled["generationConfig"]["thinkingConfig"]["thinkingBudget"] = json!(0);
        for upstream in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
        ] {
            let prepared = prepare(WireApi::Gemini, upstream, &disabled, false);
            match upstream {
                WireApi::Responses => assert_eq!(
                    prepared.upstream_body()["reasoning"],
                    json!({"effort":"none"})
                ),
                WireApi::ChatCompletions => {
                    assert_eq!(prepared.upstream_body()["reasoning_effort"], "none")
                }
                WireApi::Messages => assert_eq!(
                    prepared.upstream_body()["thinking"],
                    json!({"type":"disabled"})
                ),
                _ => unreachable!(),
            }
        }
    }
    let capabilities = crate::model_metadata::ModelCapabilities {
        reasoning: Some(false),
        ..Default::default()
    };
    let visible = request(json!(true));
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        let prepared = SourceAdapter::between(WireApi::Gemini, upstream)
            .unwrap()
            .prepare_request_with_capabilities(
                AdapterRequestContext {
                    client_wire_api: WireApi::Gemini,
                    request: &visible,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "source-test",
                    response_id_seed: "display-test",
                },
                &capabilities,
            )
            .unwrap();
        for field in ["reasoning", "reasoning_effort", "thinking", "output_config"] {
            assert!(prepared.upstream_body().get(field).is_none());
        }
    }
}

#[test]
fn gemini_display_validates_boolean_controls_and_preserves_null_default() {
    for invalid in [json!("false"), json!(0), json!(1), json!([]), json!({})] {
        let error = try_convert(
            WireApi::Gemini,
            WireApi::Responses,
            &request(invalid),
            false,
        )
        .unwrap_err();
        assert_eq!(error.parameter(), Some(PARAMETER));
    }
    let nullable = request(Value::Null);
    let mut absent = nullable.clone();
    absent["generationConfig"]["thinkingConfig"]
        .as_object_mut()
        .unwrap()
        .remove("includeThoughts");
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        let expected = prepare(WireApi::Gemini, upstream, &absent, false);
        let nullable = prepare(WireApi::Gemini, upstream, &nullable, false);
        assert_eq!(nullable.upstream_body(), expected.upstream_body());
    }
}

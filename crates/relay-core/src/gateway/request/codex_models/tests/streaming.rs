use super::route_features::{runtime_with_endpoints, runtime_with_features};
use super::*;

fn catalog(runtime: &GatewayRuntime, key: usize) -> Option<Value> {
    let key = runtime
        .authenticate_secret(&format!("synthetic-{key}"))
        .unwrap();
    build_codex_models_response(runtime, &key, &["tenant/gpt-test".into()], None)
}

#[test]
fn streaming_exclusions_respect_key_scope_without_removing_generic_inventory() {
    let runtime = runtime_with_features(
        json!({"streaming":true}),
        [
            json!({"streaming":"unsupported"}),
            json!({"streaming":"declared"}),
        ],
    );
    for (index, available) in [(0, true), (1, false), (2, true)] {
        let key = runtime
            .authenticate_secret(&format!("synthetic-{index}"))
            .unwrap();
        assert_eq!(catalog(&runtime, index).is_some(), available);
        assert!(
            !runtime
                .visible_models(&key, &[WireApi::Responses], 0)
                .is_empty()
        );
    }
}

#[test]
fn reference_streaming_exclusions_win_over_positive_endpoint_claims() {
    let runtime = runtime_with_features(
        json!({"streaming":false}),
        [
            json!({"streaming":"declared"}),
            json!({"streaming":"confirmed"}),
        ],
    );
    for index in 0..3 {
        assert!(catalog(&runtime, index).is_none());
    }
}

#[test]
fn nonstreaming_alternatives_cannot_restore_codex_media_or_parallel_flags() {
    let runtime = runtime_with_features(
        json!({
            "tool_call":true,"parallel_tool_call":true,"attachment":true,
            "modalities":{"input":["text","image"],"output":["text"]}
        }),
        [
            json!({"streaming":"unsupported","images":"declared","function_tools":"declared"}),
            json!({"streaming":"declared","images":"unsupported","function_tools":"unsupported"}),
        ],
    );
    for index in [0, 2] {
        let response = catalog(&runtime, index).unwrap();
        let model = &response["models"][0];
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["supports_parallel_tool_calls"], false);
    }
}

#[test]
fn nonstreaming_alternatives_cannot_change_codex_context_efforts_or_speed() {
    let runtime = runtime_with_endpoints(
        json!({
            "streaming":true,"reasoning":true,
            "reasoning_effort_levels":["low","high"],
            "limit":{"context":100000,"input":90000,"output":10000},
            "service_tiers":["priority"]
        }),
        [
            json!({
                "features":{"streaming":"unsupported"},"reasoningEfforts":["low"],
                "tokenLimits":{"context":1000,"input":800,"output":100},
                "serviceTiers":[DefaultServiceTier::Standard,DefaultServiceTier::Fast]
            }),
            json!({
                "features":{"streaming":"declared"},"reasoningEfforts":["high"],
                "tokenLimits":{"context":50000,"input":40000,"output":5000},
                "serviceTiers":[DefaultServiceTier::Standard]
            }),
        ],
    );
    let response = catalog(&runtime, 0).unwrap();
    let model = &response["models"][0];
    assert_eq!(model["max_context_window"], 50000);
    assert!(model["auto_compact_token_limit"].as_u64().unwrap() <= 40000);
    assert_eq!(
        model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level["effort"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["high"]
    );
    assert_eq!(model["service_tiers"], json!([]));
    assert_eq!(
        response["relay_context_limits"]["tenant/gpt-test"],
        json!({
            "context":50000,"input":40000,"output":5000
        })
    );
    let key = runtime.authenticate_secret("synthetic-0").unwrap();
    assert_eq!(
        runtime
            .token_limits_for_routes(&runtime.configured_executor_routes(
                &key,
                "gpt-test",
                &[WireApi::Responses],
                false,
            ))
            .context,
        Some(1000)
    );
    assert_eq!(
        runtime.client_reasoning_levels(&key, "gpt-test", WireApi::Responses, false),
        ["low", "high"]
    );
}

#[test]
fn managed_reasoning_placeholder_uses_key_prefix_and_stream_eligible_routes() {
    let runtime = runtime_with_endpoints(
        json!({"reasoning":true}),
        [
            json!({"features":{"streaming":"unsupported"},"reasoningEfforts":["low"]}),
            json!({"features":{"streaming":"declared"}}),
        ],
    );
    let key = runtime.authenticate_secret("synthetic-0").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "originator",
        axum::http::HeaderValue::from_static("codex_cli_rs"),
    );
    for model in [
        "tenant/gpt-test".into(),
        crate::codex_model_alias("tenant/gpt-test"),
    ] {
        for stream in [false, true] {
            let mut request = json!({
                "model":model,"stream":stream,"reasoning":{"effort":"none"}
            });
            crate::gateway::request::codex_reasoning::normalize_codex_reasoning_placeholder(
                &runtime,
                &key,
                &headers,
                &mut request,
            );
            assert_eq!(request.get("reasoning").is_none(), stream);
        }
    }
}

use super::*;
use crate::WireApi;
use serde_json::{json, Value};

fn prepare(adapter: SourceAdapter, request: &Value) -> AdapterResult<PreparedAdapterRequest> {
    adapter.prepare_request(AdapterRequestContext {
        client_wire_api: WireApi::Responses,
        request,
        model: "synthetic-model",
        stream: false,
        reasoning_mode: MessagesReasoningMode::Adaptive,
        cache_write_ttl: Default::default(),
        previous: None,
        response_scope: "synthetic-route",
        response_id_seed: "synthetic-request",
    })
}

const BRIDGES: [SourceAdapter; 3] = [
    SourceAdapter::ResponsesToChatCompletions,
    SourceAdapter::ResponsesToMessages,
    SourceAdapter::ResponsesToGemini,
];

#[test]
fn codex_transport_controls_do_not_block_inference_or_leak_to_upstream() {
    let request = json!({
        "input":"Synthetic compacted summary",
        "store":false,
        "include":["reasoning.encrypted_content"],
        "prompt_cache_key":"synthetic-affinity",
        "client_metadata":{"source":"synthetic-client"},
        "text":{"format":{"type":"text"}},
        "reasoning":{"effort":"high","summary":"auto"},
        "parallel_tool_calls":true
    });
    for adapter in BRIDGES {
        let prepared = prepare(adapter, &request).unwrap();
        let body = prepared.upstream_body();
        for field in ["include", "prompt_cache_key", "client_metadata"] {
            assert!(body.get(field).is_none(), "{adapter:?}: {field}");
        }
        assert!(body.to_string().contains("Synthetic compacted summary"));
        assert!(!body.to_string().contains("encrypted_content"));
    }
    let native = prepare(SourceAdapter::Native, &request).unwrap();
    for field in [
        "include",
        "prompt_cache_key",
        "client_metadata",
        "reasoning",
    ] {
        assert_eq!(native.upstream_body()[field], request[field]);
    }
}

#[test]
fn bridge_parameter_errors_name_only_known_fields_and_preserve_input() {
    for (extension, parameter) in [
        (json!({"text":{"verbosity":"high"}}), "text.verbosity"),
        (
            json!({"reasoning":{"summary":"detailed"}}),
            "reasoning.summary",
        ),
        (
            json!({"include":["message.output_text.logprobs"]}),
            "include",
        ),
        (json!({"store":true}), "store"),
        (json!({"background":true}), "background"),
        (
            json!({"stream_options":{"reasoning_summary_delivery":"sequential_cutoff"}}),
            "stream_options",
        ),
        (
            json!({"arbitrary-sensitive-field":"synthetic-value"}),
            "request",
        ),
        (
            json!({"input":[
                {"role":"user","content":"Synthetic history"},
                {"type":"reasoning","encrypted_content":"synthetic-opaque"}
            ]}),
            "input.encrypted_content",
        ),
    ] {
        let mut request = json!({"input":"Synthetic input"});
        request
            .as_object_mut()
            .unwrap()
            .extend(extension.as_object().unwrap().clone());
        let original = request.clone();
        for adapter in BRIDGES {
            let error = prepare(adapter, &request).unwrap_err();
            assert_eq!(
                error.code(),
                crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED
            );
            assert_eq!(error.parameter(), Some(parameter), "{adapter:?}");
            assert_eq!(request, original);
        }
        assert!(prepare(SourceAdapter::Native, &request).is_ok());
    }
}

#[test]
fn malformed_client_controls_are_not_ignored() {
    for (field, value) in [
        ("client_metadata", json!({"trace":12})),
        ("client_metadata", json!(["trace"])),
        ("include", json!("reasoning.encrypted_content")),
        ("prompt_cache_key", json!([12])),
    ] {
        let mut request = json!({"input":"Synthetic input"});
        request[field] = value;
        for adapter in BRIDGES {
            assert_eq!(
                prepare(adapter, &request).unwrap_err().code(),
                crate::error_codes::ADAPTER_INVALID_REQUEST
            );
        }
    }
}

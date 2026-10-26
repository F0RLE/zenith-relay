use super::is_managed_codex_client;
use crate::model_metadata::ReasoningMethod;
use crate::runtime::AuthenticatedKey;
use crate::{GatewayRuntime, WireApi};
use axum::http::HeaderMap;
use serde_json::Value;

/// Codex uses `none` when a Relay picker row has no effort choices. For a
/// known thinking model without published controls, that is a client placeholder,
/// not a provider-supported instruction to disable thinking. Keep explicit
/// controls and requests outside Relay's managed picker unchanged.
pub(in crate::gateway) fn normalize_codex_reasoning_placeholder(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    request: &mut Value,
) {
    if !is_managed_codex_client(headers)
        || request.pointer("/reasoning/effort").and_then(Value::as_str) != Some("none")
    {
        return;
    }
    let Some(model) = request
        .get("model")
        .and_then(Value::as_str)
        .and_then(crate::catalog::decode_codex_model_alias)
    else {
        return;
    };
    let capabilities = runtime.model_capabilities(&model);
    if capabilities.reasoning != Some(true)
        || !matches!(
            capabilities.reasoning_method,
            None | Some(ReasoningMethod::Unknown)
        )
        || !runtime
            .client_reasoning_levels(key, &model, WireApi::Responses)
            .is_empty()
    {
        return;
    }
    let Some(request_fields) = request.as_object_mut() else {
        return;
    };
    let Some(reasoning) = request_fields
        .get_mut("reasoning")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    reasoning.remove("effort");
    if reasoning.is_empty() {
        request_fields.remove("reasoning");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GatewayRuntimeOptions;
    use crate::gateway::request::tests::capability_test_runtime;
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
    use axum::http::HeaderValue;
    use serde_json::json;

    fn runtime() -> GatewayRuntime {
        let catalog = ModelMetadataCatalog::from_models_dev_json(&json!({
            "vendor/fixed-thinking": {"reasoning": true},
            "another-provider/automatic": {"reasoning": true, "reasoning_method": "unknown"},
            "vendor/adjustable": {"reasoning": true, "reasoning_effort_levels": ["low", "high"]},
            "vendor/toggle": {"reasoning": true, "reasoning_method": "toggle"},
            "vendor/plain": {"reasoning": false}
        }).to_string()).unwrap();
        capability_test_runtime(
            &[
                "vendor/fixed-thinking",
                "another-provider/automatic",
                "vendor/adjustable",
                "vendor/toggle",
                "vendor/plain",
                "vendor/unknown",
            ],
            GatewayRuntimeOptions {
                model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(catalog)),
                ..GatewayRuntimeOptions::default()
            },
        )
    }

    #[test]
    fn codex_placeholder_uses_catalog_evidence_and_preserves_other_fields() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        for model in ["vendor/fixed-thinking", "another-provider/automatic"] {
            for reasoning in [
                json!({"effort":"none"}),
                json!({"effort":"none", "summary":"auto"}),
            ] {
                let mut request = json!({"model":crate::codex_model_alias(model), "input":"hello", "reasoning":reasoning});
                let mut expected = request.clone();
                expected["reasoning"]
                    .as_object_mut()
                    .unwrap()
                    .remove("effort");
                if expected["reasoning"].as_object().unwrap().is_empty() {
                    expected.as_object_mut().unwrap().remove("reasoning");
                }
                normalize_codex_reasoning_placeholder(&runtime, &key, &headers, &mut request);
                assert_eq!(request, expected);
            }
        }
    }

    #[test]
    fn codex_placeholder_does_not_override_explicit_controls_or_unknown_models() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        for model in [
            "vendor/adjustable",
            "vendor/toggle",
            "vendor/plain",
            "vendor/unknown",
        ] {
            let mut request =
                json!({"model":crate::codex_model_alias(model), "reasoning":{"effort":"none"}});
            let original = request.clone();
            normalize_codex_reasoning_placeholder(&runtime, &key, &headers, &mut request);
            assert_eq!(request, original, "{model}");
        }
        for (model, effort, managed) in [
            ("vendor/fixed-thinking".to_string(), "none", true),
            (
                crate::codex_model_alias("vendor/fixed-thinking"),
                "high",
                true,
            ),
            (
                crate::codex_model_alias("vendor/fixed-thinking"),
                "none",
                false,
            ),
        ] {
            let mut request = json!({"model":model, "reasoning":{"effort":effort}});
            let original = request.clone();
            let client_headers = if managed {
                headers.clone()
            } else {
                HeaderMap::new()
            };
            normalize_codex_reasoning_placeholder(&runtime, &key, &client_headers, &mut request);
            assert_eq!(request, original);
        }
    }
}

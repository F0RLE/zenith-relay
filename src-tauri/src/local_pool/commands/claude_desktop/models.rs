use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use serde_json::{json, Value};
use zenith_relay_core::protocol::ModelSummary;

const MAX_INFERENCE_MODELS: usize = 200;

/// A Desktop cap is not an effort enum and cannot remove intermediate holes.
/// Keep supported higher choices visible; admission rejects unsupported values
/// without silently replacing the user's selected effort.
fn native_effort_cap(model: &ModelSummary) -> Option<&'static str> {
    ["low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .rfind(|effort| {
            model
                .reasoning_allowed_levels
                .iter()
                .any(|level| level == *effort)
                && model.protocol_routes.iter().any(|route| {
                    route.client_wire_api == zenith_relay_core::WireApi::Messages
                        && route.reasoning_efforts.iter().any(|level| level == *effort)
                })
        })
}

pub(super) fn inference_models(models: &[ModelSummary]) -> LocalResult<Vec<Value>> {
    // The management projection already applies membership, visibility and pool order.
    // Expose enabled models with an executable Messages-facing route, including
    // bridges to every supported native upstream. The gateway routes
    // Messages-native models as Native (passthrough) and bridges non-native
    // models through the appropriate SourceAdapter automatically.
    // Effort belongs to the request, not model identity. Use a Claude UI route
    // only for a representable native scale, without changing the real model.
    let entries: Vec<_> = models
        .iter()
        .filter(|model| {
            model.enabled
                && model
                    .protocol_routes
                    .iter()
                    .any(|route| route.client_wire_api == zenith_relay_core::WireApi::Messages)
        })
        .map(|model| {
            let label = model.catalog_name.as_deref().unwrap_or(&model.id);
            let cap = native_effort_cap(model);
            let name = if cap.is_some() {
                zenith_relay_core::claude_desktop_reasoning_model_alias(&model.id)
            } else {
                zenith_relay_core::claude_desktop_model_alias(&model.id)
            };
            let mut entry = json!({
                "name": name,
                "labelOverride": label,
            });
            if let Some(cap) = cap {
                entry["maxEffort"] = json!(cap);
            }
            entry
        })
        .collect();
    if entries.len() > MAX_INFERENCE_MODELS {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Claude Desktop supports at most 200 models; reduce enabled pool models before connecting. The existing profile was kept",
        ));
    }
    Ok(entries)
}

pub(super) fn gateway_config(origin: &str, secret: &str, models: Vec<Value>) -> Value {
    json!({
        "inferenceProvider": "gateway",
        "inferenceCredentialKind": "static",
        "inferenceGatewayAuthScheme": "bearer",
        "inferenceGatewayBaseUrl": origin,
        "inferenceGatewayApiKey": secret,
        "modelDiscoveryEnabled": false,
        "inferenceModels": models,
        // Desktop otherwise eagerly loads every MCP schema on custom gateways.
        // Relay bridges client-owned discovery; native Messages stays native.
        "toolSearchEnabled": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::WireApi;

    #[test]
    fn three_reasoning_models_remain_three_model_choices() {
        let models: Vec<_> = ["vendor/first", "vendor/second", "vendor/third"]
            .into_iter()
            .map(|id| {
                let mut configured = model(id, true, WireApi::Messages);
                let levels: Vec<String> = ["low", "medium", "high", "xhigh", "max", "ultra"]
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                configured.reasoning_allowed_levels = levels.clone();
                configured.protocol_routes[0].reasoning_efforts = levels;
                configured
            })
            .collect();
        let entries = inference_models(&models).unwrap();
        assert_eq!(entries.len(), 3);
        for (entry, model) in entries.iter().zip(&models) {
            assert_eq!(
                entry["name"],
                zenith_relay_core::claude_desktop_reasoning_model_alias(&model.id)
            );
            assert_eq!(entry["labelOverride"], model.id);
            assert!(entry.get("anthropicFamilyTier").is_none());
            assert_eq!(entry["maxEffort"], "max");
        }
    }

    #[test]
    fn native_effort_choices_are_capped_by_both_pool_policy_and_messages_routes() {
        let mut configured = model("upstream/reasoning", true, WireApi::Messages);
        configured.reasoning_allowed_levels =
            vec!["low".into(), "medium".into(), "high".into(), "ultra".into()];
        configured.protocol_routes[0].reasoning_efforts = vec!["low".into(), "medium".into()];
        assert_eq!(
            inference_models(std::slice::from_ref(&configured)).unwrap()[0]["maxEffort"],
            "medium"
        );

        configured.reasoning_allowed_levels = vec!["low".into(), "high".into()];
        configured.protocol_routes[0].reasoning_efforts =
            vec!["low".into(), "medium".into(), "high".into()];
        assert_eq!(
            inference_models(std::slice::from_ref(&configured)).unwrap()[0]["maxEffort"],
            "high"
        );

        let mut responses = configured.protocol_routes[0].clone();
        responses.client_wire_api = WireApi::Responses;
        configured.protocol_routes[0].reasoning_efforts = vec!["low".into()];
        configured.protocol_routes.push(responses);
        configured.reasoning_allowed_levels.push("medium".into());
        assert_eq!(
            inference_models(&[configured]).unwrap()[0]["maxEffort"],
            "low"
        );
    }

    #[test]
    fn unknown_or_unsupported_native_reasoning_does_not_advertise_a_picker() {
        for levels in [vec![], vec!["none".into(), "ultra".into()]] {
            let mut configured = model("upstream/model", true, WireApi::Messages);
            configured.reasoning_allowed_levels = levels;
            configured.protocol_routes[0].reasoning_efforts =
                vec!["low".into(), "medium".into(), "high".into(), "ultra".into()];
            let entries = inference_models(&[configured]).unwrap();
            assert_eq!(
                entries[0]["name"],
                zenith_relay_core::claude_desktop_model_alias("upstream/model")
            );
            assert!(entries[0].get("maxEffort").is_none());
            assert!(entries[0].get("supports1m").is_none());
        }
    }

    #[test]
    fn sparse_efforts_do_not_hide_high_or_max_from_desktop() {
        for levels in [
            vec!["high"],
            vec!["high", "max"],
            vec!["none", "low", "high", "max"],
        ] {
            let mut configured = model("upstream/sparse", true, WireApi::Messages);
            configured.reasoning_allowed_levels =
                levels.iter().map(|level| (*level).into()).collect();
            configured.protocol_routes[0].reasoning_efforts =
                configured.reasoning_allowed_levels.clone();
            let entry = &inference_models(&[configured]).unwrap()[0];
            assert_eq!(entry["maxEffort"], *levels.last().unwrap());
            assert_eq!(
                entry["name"],
                zenith_relay_core::claude_desktop_reasoning_model_alias("upstream/sparse")
            );
        }
    }

    fn model(id: &str, enabled: bool, protocol: WireApi) -> ModelSummary {
        serde_json::from_value(json!({
            "id": id, "enabled": enabled, "memberCount": 1,
            "inputMicroUsdPerMillion": null, "cachedInputMicroUsdPerMillion": null,
            "outputMicroUsdPerMillion": null,
            "protocolRoutes": [{"clientWireApi": protocol, "upstreamWireApi": protocol}]
        }))
        .unwrap()
    }

    #[test]
    fn uses_desktop_route_ids_but_keeps_real_labels_and_pool_order() {
        let mut first = model("provider/gpt-test", true, WireApi::Messages);
        first.catalog_name = Some("Pool model".into());
        let models = inference_models(&[
            first,
            model("disabled", false, WireApi::Messages),
            model("responses-only", true, WireApi::Responses),
            model("claude-test", true, WireApi::Messages),
        ])
        .unwrap();
        assert_eq!(
            models,
            vec![
                json!({"name": zenith_relay_core::claude_desktop_model_alias("provider/gpt-test"), "labelOverride": "Pool model"}),
                json!({"name": zenith_relay_core::claude_desktop_model_alias("claude-test"), "labelOverride": "claude-test"}),
            ]
        );
    }

    #[test]
    fn includes_bridged_models_but_not_unroutable_inventory() {
        let mut bridged = model("openai/test", true, WireApi::Messages);
        bridged.protocol_routes[0].upstream_wire_api = WireApi::Responses;
        let mut unroutable = model("unroutable", true, WireApi::Messages);
        unroutable.protocol_routes.clear();
        assert_eq!(
            inference_models(&[bridged, unroutable]).unwrap(),
            vec![json!({
                "name": zenith_relay_core::claude_desktop_model_alias("openai/test"), "labelOverride": "openai/test"
            })]
        );
    }

    #[test]
    fn rejects_oversized_desktop_catalog_without_silently_dropping_models() {
        let mut models: Vec<_> = (0..MAX_INFERENCE_MODELS)
            .map(|index| {
                let mut entry = model(&format!("model-{index}"), true, WireApi::Messages);
                entry.reasoning_allowed_levels = vec!["high".into()];
                entry.protocol_routes[0].reasoning_efforts = vec!["high".into()];
                entry
            })
            .collect();
        assert_eq!(
            inference_models(&models).unwrap().len(),
            MAX_INFERENCE_MODELS
        );
        models.push(model("one-too-many", true, WireApi::Messages));
        assert!(inference_models(&models).is_err());
    }

    #[test]
    fn writes_static_catalog_without_claiming_family_or_context_capabilities() {
        let config = gateway_config("http://127.0.0.1:14998", "synthetic", vec![]);
        assert_eq!(config["modelDiscoveryEnabled"], false);
        assert_eq!(config["inferenceGatewayAuthScheme"], "bearer");
        assert_eq!(config["inferenceGatewayBaseUrl"], "http://127.0.0.1:14998");
        assert_eq!(config["inferenceModels"], json!([]));
        assert_eq!(config["toolSearchEnabled"], true);
        // Desktop's modelCatalogUrl requires Anthropic-signed bytes; do not
        // configure a local unsigned catalog that the client will ignore.
        assert!(config.get("modelCatalogUrl").is_none());
        assert!(config.get("anthropicFamilyTier").is_none());
    }
}

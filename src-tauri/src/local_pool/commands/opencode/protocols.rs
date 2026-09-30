use super::*;

mod merge;
#[cfg(test)]
use merge::merge_provider;
use merge::{existing_protocol, merge_groups, provider_with_models, set_variants};
use std::collections::BTreeMap;

pub(super) const GROUPS: [(WireApi, &str, &str); 4] = [
    (WireApi::Responses, PROVIDER_ID, PROVIDER_NPM),
    (
        WireApi::ChatCompletions,
        "zenith-relay-chat",
        "@ai-sdk/openai-compatible",
    ),
    (
        WireApi::Messages,
        "zenith-relay-messages",
        "@ai-sdk/anthropic",
    ),
    (WireApi::Gemini, "zenith-relay-gemini", "@ai-sdk/google"),
];

pub(super) fn managed_id(id: &str) -> bool {
    GROUPS.iter().any(|(_, candidate, _)| id == *candidate)
}

pub(super) fn managed_model(model: &str) -> bool {
    model.split_once('/').is_some_and(|(id, _)| managed_id(id))
}

fn supports(model: &ModelSummary, protocol: WireApi) -> bool {
    model
        .protocol_routes
        .iter()
        .any(|route| route.client_wire_api == protocol)
}

pub(super) fn preferred(model: &ModelSummary) -> WireApi {
    WireApi::ALL
        .into_iter()
        .find(|protocol| {
            model.protocol_routes.iter().any(|route| {
                route.client_wire_api == *protocol && route.upstream_wire_api == *protocol
            })
        })
        .or_else(|| {
            WireApi::ALL
                .into_iter()
                .find(|protocol| supports(model, *protocol))
        })
        .unwrap_or(WireApi::Responses)
}

pub(super) fn base_url(base: &str, protocol: WireApi) -> Result<String, LocalPoolError> {
    let mut url = url::Url::parse(base)
        .map_err(|_| LocalPoolError::invalid_state("invalid provider address"))?;
    if protocol == WireApi::Gemini {
        let path = url.path().trim_end_matches('/');
        let prefix = path
            .strip_suffix("/v1")
            .or_else(|| path.strip_suffix("/v1beta"))
            .unwrap_or(path);
        url.set_path(&format!("{prefix}/v1beta"));
    }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

pub(super) fn provider(
    base: &str,
    secret: &str,
    models: &[ModelSummary],
    protocol: WireApi,
) -> Result<Value, LocalPoolError> {
    let mut configured = model_config(models);
    for (id, value) in &mut configured {
        let model = models.iter().find(|model| model.id == *id).unwrap();
        if !model.protocol_routes.is_empty() {
            let routes = model
                .protocol_routes
                .iter()
                .filter(|route| route.client_wire_api == protocol)
                .collect::<Vec<_>>();
            if let Some(variants) = value.get_mut("variants").and_then(Value::as_object_mut) {
                variants.retain(|effort, _| {
                    routes
                        .iter()
                        .any(|route| route.reasoning_efforts.contains(effort))
                });
            }
            for (key, feature) in [
                (
                    "tool_call",
                    zenith_relay_core::ProtocolFeature::FunctionTools,
                ),
                ("reasoning", zenith_relay_core::ProtocolFeature::Reasoning),
            ] {
                if routes.iter().all(|route| {
                    route.features.get(&feature)
                        == Some(&zenith_relay_core::CapabilityStatus::Unsupported)
                }) {
                    value[key] = false.into();
                }
            }
        }
        set_variants(value, protocol);
    }
    provider_with_models(base, secret, configured, protocol)
}

mod apply;

pub(in crate::local_pool::commands::opencode) use apply::{apply, apply_source};
#[cfg(test)]
mod tests;

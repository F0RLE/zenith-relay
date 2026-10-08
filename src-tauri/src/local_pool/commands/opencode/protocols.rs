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

pub(super) fn managed_id(provider_id: &str) -> bool {
    GROUPS
        .iter()
        .any(|(_, candidate_provider_id, _)| provider_id == *candidate_provider_id)
}

pub(super) fn managed_model(model: &str) -> bool {
    model
        .split_once('/')
        .is_some_and(|(provider_id, _)| managed_id(provider_id))
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
    for (model_id, provider_model_config) in &mut configured {
        let model_record = models.iter().find(|model| model.id == *model_id).unwrap();
        if !model_record.protocol_routes.is_empty() {
            let routes = model_record
                .protocol_routes
                .iter()
                .filter(|route| route.client_wire_api == protocol)
                .collect::<Vec<_>>();
            if let Some(variants) = provider_model_config
                .get_mut("variants")
                .and_then(Value::as_object_mut)
            {
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
                    provider_model_config[key] = false.into();
                }
            }
        }
        set_variants(provider_model_config, protocol);
    }
    provider_with_models(base, secret, configured, protocol)
}

mod apply;

pub(in crate::local_pool::commands::opencode) use apply::{apply, apply_source};
#[cfg(test)]
mod tests;

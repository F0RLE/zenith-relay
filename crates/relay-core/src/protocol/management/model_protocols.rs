use super::{AccountSummary, ModelSummary, SourceSummary};
use crate::{
    CapabilityStatus, MessagesReasoningMode, ModelRules, ProtocolFeature, SourceAdapter, WireApi,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Executable paths, independent from advisory model metadata and prices.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProtocolRoute {
    pub client_wire_api: WireApi,
    pub upstream_wire_api: WireApi,
    #[serde(default)]
    pub features: BTreeMap<ProtocolFeature, CapabilityStatus>,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
}

impl ModelProtocolRoute {
    pub(super) fn project_reasoning(&mut self, fallback: &[String]) {
        if self.features.get(&ProtocolFeature::Reasoning) == Some(&CapabilityStatus::Unsupported) {
            self.reasoning_efforts.clear();
            return;
        }
        if self.reasoning_efforts.is_empty() {
            self.reasoning_efforts = fallback.to_vec();
        }
        let adapter = SourceAdapter::between(self.client_wire_api, self.upstream_wire_api);
        self.reasoning_efforts.retain(|effort| {
            adapter.is_some_and(|adapter| {
                adapter.supports_reasoning_effort(MessagesReasoningMode::Adaptive, effort)
            })
        });
        self.reasoning_efforts =
            crate::canonicalize_reasoning_levels(std::mem::take(&mut self.reasoning_efforts));
    }
}

pub fn model_protocol_routes(
    model: &str,
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
) -> Vec<ModelProtocolRoute> {
    let mut routes = Vec::new();
    for source in sources.iter().filter(|source| {
        source.enabled && source.in_pool && !source.draining && source.secret_available
    }) {
        if !allowed(model, &source.allowed_models, &source.excluded_models) {
            continue;
        }
        let capabilities = source.protocol_config.effective_capabilities(
            &source.base_url,
            &source.models,
            &source.protocol_bindings,
            source.wire_api,
        );
        for binding in source
            .protocol_config
            .resolve(
                &source.base_url,
                &source.models,
                &source.protocol_bindings,
                source.wire_api,
            )
            .unwrap_or_default()
        {
            if !binding
                .model_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(model))
            {
                continue;
            }
            let upstream = binding
                .adapter
                .upstream_protocol(binding.wire_api)
                .wire_api();
            let capability = capabilities.iter().find(|entry| {
                entry.model_id.eq_ignore_ascii_case(model) && entry.upstream_wire_api == upstream
            });
            if capability.is_some_and(|entry| {
                entry.status == CapabilityStatus::Unsupported
                    || entry.features.get(&ProtocolFeature::Text)
                        == Some(&CapabilityStatus::Unsupported)
            }) {
                continue;
            }
            routes.push(ModelProtocolRoute {
                client_wire_api: binding.wire_api,
                upstream_wire_api: upstream,
                features: capability
                    .map(|entry| entry.features.clone())
                    .unwrap_or_default(),
                reasoning_efforts: capability
                    .map(|entry| entry.reasoning_efforts.clone())
                    .unwrap_or_default(),
            });
        }
    }
    if accounts.iter().any(|account| {
        account.enabled
            && account.in_pool
            && !account.draining
            && account.secret_available
            && account.proxy_available
            && allowed(model, &account.allowed_models, &account.excluded_models)
            && account
                .models
                .iter()
                .any(|id| id.eq_ignore_ascii_case(model))
    }) {
        for client in WireApi::ALL {
            routes.push(ModelProtocolRoute {
                client_wire_api: client,
                upstream_wire_api: WireApi::Responses,
                features: BTreeMap::from([
                    (ProtocolFeature::Text, CapabilityStatus::Declared),
                    (ProtocolFeature::Streaming, CapabilityStatus::Declared),
                ]),
                reasoning_efforts: Vec::new(),
            });
        }
    }
    let mut unique = Vec::new();
    for route in routes {
        if !unique.contains(&route) {
            unique.push(route);
        }
    }
    unique
}

fn allowed(model: &str, allowed: &[String], excluded: &[String]) -> bool {
    ModelRules {
        allowed: allowed.iter().cloned().collect(),
        excluded: excluded.iter().cloned().collect(),
    }
    .allows(model)
}

pub fn codex_catalog_supports_websockets(models: &[ModelSummary]) -> bool {
    !models
        .iter()
        .filter(|model| model.enabled && model.codex_visible)
        .any(|model| {
            model.protocol_routes.iter().any(|route| {
                route.client_wire_api == WireApi::Responses
                    && route.upstream_wire_api != WireApi::Responses
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reasoning_projection_uses_the_selected_route_and_honors_explicit_unsupported() {
        let fallback = ["minimal", "low", "high", "max", "ultra"].map(str::to_owned);
        for (upstream, expected) in [
            (
                WireApi::Responses,
                vec!["minimal", "low", "high", "max", "ultra"],
            ),
            (WireApi::Messages, vec!["low", "high", "max"]),
            (WireApi::Gemini, vec!["minimal", "low", "high"]),
        ] {
            let mut route = ModelProtocolRoute {
                client_wire_api: WireApi::Responses,
                upstream_wire_api: upstream,
                features: BTreeMap::new(),
                reasoning_efforts: Vec::new(),
            };
            route.project_reasoning(&fallback);
            assert_eq!(route.reasoning_efforts, expected);
            route
                .features
                .insert(ProtocolFeature::Reasoning, CapabilityStatus::Unsupported);
            route.project_reasoning(&fallback);
            assert!(route.reasoning_efforts.is_empty());
        }
    }

    #[test]
    fn codex_disables_websockets_when_a_visible_model_has_a_converted_fallback() {
        let mut model: ModelSummary = serde_json::from_value(
            json!({"id":"test","enabled":true,"memberCount":1,"codexVisible":true}),
        )
        .unwrap();
        let route = ModelProtocolRoute {
            client_wire_api: WireApi::Responses,
            upstream_wire_api: WireApi::Responses,
            features: BTreeMap::new(),
            reasoning_efforts: Vec::new(),
        };
        model.protocol_routes.push(route.clone());
        assert!(codex_catalog_supports_websockets(&[model.clone()]));
        model.protocol_routes.push(ModelProtocolRoute {
            upstream_wire_api: WireApi::Messages,
            ..route
        });
        assert!(!codex_catalog_supports_websockets(&[model.clone()]));
        model.enabled = false;
        assert!(codex_catalog_supports_websockets(&[model.clone()]));
        model.enabled = true;
        model.codex_visible = false;
        assert!(codex_catalog_supports_websockets(&[model]));
    }
}

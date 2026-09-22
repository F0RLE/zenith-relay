use super::{AccountSummary, ModelSummary, SourceSummary};
use crate::{
    CapabilityStatus, MessagesReasoningMode, ModelRules, ProtocolFeature, SourceAdapter, WireApi,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

/// Resolve each member once for a whole catalog projection. The index lives
/// only for this snapshot, so policy edits cannot leave cached routes behind.
#[derive(Default)]
pub(super) struct ModelProtocolIndex {
    routes: BTreeMap<String, Vec<ModelProtocolRoute>>,
    cache_write_models: BTreeSet<String>,
}

impl ModelProtocolIndex {
    pub(super) fn new(sources: &[SourceSummary], accounts: &[AccountSummary]) -> Self {
        let mut index = Self::default();
        for source in sources {
            let enabled =
                source.enabled && source.in_pool && !source.draining && source.secret_available;
            let rules = ModelRules {
                allowed: source.allowed_models.iter().cloned().collect(),
                excluded: source.excluded_models.iter().cloned().collect(),
            };
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
                let upstream = binding
                    .adapter
                    .upstream_protocol(binding.wire_api)
                    .wire_api();
                for model in binding.model_ids {
                    let key = model.to_ascii_lowercase();
                    // Editable inventory keeps its price fields while a member
                    // is offline or excluded; this does not grant a route.
                    if upstream == WireApi::Messages {
                        index.cache_write_models.insert(key.clone());
                    }
                    if enabled && rules.allows(&model) {
                        index.insert(key, binding.wire_api, upstream);
                    }
                }
            }
        }
        for account in accounts.iter().filter(|account| {
            account.enabled
                && account.in_pool
                && !account.draining
                && account.secret_available
                && account.proxy_available
        }) {
            let rules = ModelRules {
                allowed: account.allowed_models.iter().cloned().collect(),
                excluded: account.excluded_models.iter().cloned().collect(),
            };
            for model in account.models.iter().filter(|model| rules.allows(model)) {
                for client in WireApi::ALL {
                    index.insert(model.to_ascii_lowercase(), client, WireApi::Responses);
                }
            }
        }
        index
    }

    fn insert(&mut self, model: String, client: WireApi, upstream: WireApi) {
        let routes = self.routes.entry(model).or_default();
        if routes
            .iter()
            .any(|route| route.client_wire_api == client && route.upstream_wire_api == upstream)
        {
            return;
        }
        routes.push(ModelProtocolRoute {
            client_wire_api: client,
            upstream_wire_api: upstream,
            // Reference metadata fills semantic fields after physical paths.
            features: BTreeMap::new(),
            reasoning_efforts: Vec::new(),
        });
    }

    pub(super) fn routes_for(&self, model: &str) -> Vec<ModelProtocolRoute> {
        self.routes
            .get(&model.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn has_cache_write_pricing(&self, model: &str) -> bool {
        self.cache_write_models
            .contains(&model.to_ascii_lowercase())
    }
}

pub fn apply_model_protocol_routes(
    models: &mut [ModelSummary],
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
) {
    let index = ModelProtocolIndex::new(sources, accounts);
    for model in models {
        model.protocol_routes = index.routes_for(&model.id);
    }
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
    fn catalog_projection_preserves_filters_and_offline_cache_prices() {
        let ids = (0..256).map(|i| format!("model-{i}")).collect::<Vec<_>>();
        let source: SourceSummary = serde_json::from_value(json!({
            "id":"source", "name":"Synthetic", "enabled":true, "inPool":true,
            "draining":false, "operationalStatus":"rotation",
            "baseUrl":"https://example.test/v1", "wireApi":"responses",
            "models":ids, "allowedModels":[], "excludedModels":["MODEL-4"],
            "priority":0, "weight":1, "secretAvailable":true,
            "protocolConfig":{"capabilities":ids.iter().enumerate().map(|(i, id)| json!({
                "modelId":id, "upstreamWireApi":if i % 2 == 0 { "messages" } else { "responses" },
                "status":"declared", "origin":"catalog", "checkedAtMs":1
            })).collect::<Vec<_>>()}
        }))
        .unwrap();
        let mut offline = source.clone();
        offline.id = "offline".into();
        offline.enabled = false;
        offline.models = vec!["offline-model".into()];
        offline.protocol_config =
            crate::SourceProtocolConfig::automatic("https://example.test/v1/messages");
        let mut duplicate = source.clone();
        duplicate.id = "duplicate".into();
        let mut models = ids
            .iter()
            .chain(&offline.models)
            .map(|id| {
                serde_json::from_value(json!({"id":id,"enabled":true,"memberCount":1})).unwrap()
            })
            .collect::<Vec<ModelSummary>>();
        let prices = BTreeMap::from([(
            "offline-model".into(),
            crate::ApiModelPriceOverride {
                input_micro_usd_per_million: 1_000_000,
                cached_input_micro_usd_per_million: None,
                cache_write_5m_micro_usd_per_million: Some(1_250_000),
                cache_write_1h_micro_usd_per_million: Some(2_000_000),
                output_micro_usd_per_million: 5_000_000,
            },
        )]);
        super::super::apply_pool_model_configuration(
            &mut models,
            &[source, offline, duplicate],
            &[],
            &prices,
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
        );
        assert_eq!(models.len(), ids.len() + 1);
        for (i, model) in models[..ids.len()].iter().enumerate() {
            if i == 4 {
                assert!(model.protocol_routes.is_empty());
                assert!(!model.codex_visible);
                continue;
            }
            assert_eq!(model.protocol_routes.len(), 4);
            assert!(model
                .protocol_routes
                .iter()
                .all(|route| route.upstream_wire_api
                    == if i % 2 == 0 {
                        WireApi::Messages
                    } else {
                        WireApi::Responses
                    }));
        }
        let offline = models.last().unwrap();
        assert!(offline.protocol_routes.is_empty());
        assert!(!offline.codex_visible);
        assert_eq!(
            offline.cache_write_5m_micro_usd_per_million,
            Some(1_250_000)
        );
        assert_eq!(
            offline.cache_write_1h_micro_usd_per_million,
            Some(2_000_000)
        );
    }

    #[test]
    fn model_projection_collects_paths_without_participant_semantic_fields() {
        let source: SourceSummary = serde_json::from_value(json!({
            "id":"source", "name":"Synthetic", "enabled":true, "inPool":true,
            "draining":false, "operationalStatus":"rotation",
            "baseUrl":"https://example.test/v1", "wireApi":"responses",
            "models":["future-model"], "allowedModels":[], "excludedModels":[],
            "priority":0, "weight":1, "secretAvailable":true,
            "protocolConfig":{"capabilities":[{
                "modelId":"future-model", "upstreamWireApi":"responses",
                "status":"unsupported", "origin":"catalog", "checkedAtMs":1,
                "features":{"text":"unsupported", "reasoning":"unsupported"}
            }]}
        }))
        .unwrap();
        let expected = source
            .protocol_config
            .resolve(
                &source.base_url,
                &source.models,
                &source.protocol_bindings,
                source.wire_api,
            )
            .unwrap();
        let mut other = source.clone();
        other.id = "second-source".into();
        other.protocol_config.capabilities.clear();
        let index = ModelProtocolIndex::new(&[source, other], &[]);
        let routes = index.routes_for("future-model");
        assert_eq!(routes, index.routes_for("FUTURE-MODEL"));
        assert_eq!(routes.len(), expected.len());
        for route in routes {
            assert!(expected.iter().any(|binding| {
                binding.wire_api == route.client_wire_api
                    && binding
                        .adapter
                        .upstream_protocol(binding.wire_api)
                        .wire_api()
                        == route.upstream_wire_api
            }));
            assert!(route.features.is_empty());
            assert!(route.reasoning_efforts.is_empty());
        }
    }

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

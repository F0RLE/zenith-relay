use super::{SourceProtocolBinding, WireApi};
use crate::{CacheWriteTtl, MessagesReasoningMode, Result, SourceAdapter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Declared,
    Confirmed,
    Unsupported,
    #[default]
    Unknown,
}

impl CapabilityStatus {
    pub const fn available(self) -> bool {
        matches!(self, Self::Declared | Self::Confirmed)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOrigin {
    Catalog,
    ServiceProfile,
    EndpointUrl,
    Manual,
    GenerationProbe,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFeature {
    Text,
    Streaming,
    Images,
    FunctionTools,
    ToolChoice,
    StructuredOutput,
    Reasoning,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEndpointCapability {
    pub model_id: String,
    pub upstream_wire_api: WireApi,
    pub status: CapabilityStatus,
    pub origin: CapabilityOrigin,
    pub checked_at_ms: u64,
    #[serde(default)]
    pub features: BTreeMap<ProtocolFeature, CapabilityStatus>,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProtocolConfig {
    /// Incremented whenever the address or credential changes. Probe results
    /// are applied only to the revision they actually tested.
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub capabilities: Vec<ModelEndpointCapability>,
    #[serde(default)]
    pub endpoint_hint: Option<WireApi>,
}

impl SourceProtocolConfig {
    pub fn effective_capabilities(
        &self,
        base_url: &str,
        models: &[String],
    ) -> Vec<ModelEndpointCapability> {
        let hint = self
            .endpoint_hint
            .or_else(|| endpoint_url_protocol(base_url));
        let profile = service_protocol(base_url);
        let mut indexed = BTreeMap::<_, Vec<_>>::new();
        for entry in &self.capabilities {
            // Legacy generation probes are diagnostic records, not routing or
            // model-capability evidence. Never let them override discovery.
            if entry.origin == CapabilityOrigin::GenerationProbe {
                continue;
            }
            if entry.status == CapabilityStatus::Unknown
                && (entry.origin != CapabilityOrigin::Catalog
                    || (entry.features.is_empty() && entry.reasoning_efforts.is_empty()))
            {
                continue;
            }
            indexed
                .entry((entry.model_id.to_ascii_lowercase(), entry.upstream_wire_api))
                .or_default()
                .push(entry);
        }
        for observations in indexed.values_mut() {
            observations.sort_by_key(|entry| entry.checked_at_ms);
        }
        let mut result = Vec::new();
        for model in models {
            let model_key = model.to_ascii_lowercase();
            for upstream in WireApi::ALL {
                let observations = indexed
                    .get(&(model_key.clone(), upstream))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let declaration = if hint == Some(upstream) {
                    Some(CapabilityOrigin::EndpointUrl)
                } else if hint.is_none() && profile == Some(upstream) {
                    Some(CapabilityOrigin::ServiceProfile)
                } else {
                    None
                };
                let mut merged = declaration
                    .map(|origin| ModelEndpointCapability {
                        model_id: model.clone(),
                        upstream_wire_api: upstream,
                        status: CapabilityStatus::Declared,
                        origin,
                        checked_at_ms: 0,
                        features: BTreeMap::from([(
                            ProtocolFeature::Text,
                            CapabilityStatus::Declared,
                        )]),
                        reasoning_efforts: vec![],
                    })
                    .or_else(|| observations.first().map(|entry| (*entry).clone()));
                if let Some(entry) = merged.as_mut() {
                    for observation in observations {
                        // Feature-only catalog rows do not establish or erase
                        // the protocol selected by an endpoint declaration.
                        if observation.status != CapabilityStatus::Unknown {
                            entry.status = observation.status;
                            entry.origin = observation.origin;
                            entry.checked_at_ms = observation.checked_at_ms;
                        }
                        entry.features.extend(observation.features.clone());
                        if !observation.reasoning_efforts.is_empty() {
                            entry.reasoning_efforts = observation.reasoning_efforts.clone();
                        }
                    }
                }
                result.extend(merged);
            }
        }
        result
    }

    pub fn with_effective_capabilities(&self, base_url: &str, models: &[String]) -> Self {
        Self {
            capabilities: self.effective_capabilities(base_url, models),
            ..self.clone()
        }
    }

    pub fn models_for(
        &self,
        base_url: &str,
        models: &[String],
        bindings: &[SourceProtocolBinding],
        fallback: WireApi,
        client: Option<WireApi>,
    ) -> Result<Vec<String>> {
        let routes = self.resolve(base_url, models, bindings, fallback)?;
        let routed_models = routes
            .into_iter()
            .filter(|route| client.is_none_or(|client| route.wire_api == client))
            .flat_map(|route| route.model_ids)
            .map(|model| model.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        Ok(crate::normalize_model_ids(
            models
                .iter()
                .filter(|model| routed_models.contains(&model.to_ascii_lowercase()))
                .cloned()
                .collect::<Vec<_>>(),
        ))
    }

    pub fn automatic(base_url: &str) -> Self {
        Self {
            endpoint_hint: endpoint_url_protocol(base_url),
            ..Self::default()
        }
    }

    pub fn invalidate(&mut self, base_url: &str) {
        self.revision = self.revision.saturating_add(1);
        self.capabilities.clear();
        self.endpoint_hint = endpoint_url_protocol(base_url);
    }

    pub fn apply_probe(&mut self, revision: u64, observation: ModelEndpointCapability) -> bool {
        if self.revision != revision || observation.origin != CapabilityOrigin::GenerationProbe {
            return false;
        }
        // Inconclusive attempts do not erase earlier generation evidence.
        if observation.status == CapabilityStatus::Unknown {
            return true;
        }
        self.capabilities.retain(|existing| {
            existing.origin != CapabilityOrigin::GenerationProbe
                || existing.upstream_wire_api != observation.upstream_wire_api
                || !existing
                    .model_id
                    .eq_ignore_ascii_case(&observation.model_id)
        });
        self.capabilities.push(observation);
        true
    }

    pub fn merge_catalog(&mut self, observations: Vec<ModelEndpointCapability>) {
        self.capabilities
            .retain(|observation| observation.origin != CapabilityOrigin::Catalog);
        self.capabilities.extend(observations);
    }

    /// Resolve every catalog model. Catalog declarations and configured
    /// endpoint identity select the upstream wire format. Legacy generation
    /// probes are diagnostic only. Names, families and prices never
    /// participate.
    pub fn resolve(
        &self,
        base_url: &str,
        models: &[String],
        legacy_bindings: &[SourceProtocolBinding],
        fallback: WireApi,
    ) -> Result<Vec<SourceProtocolBinding>> {
        let mut capabilities = BTreeMap::<_, Vec<_>>::new();
        for capability in self.effective_capabilities(base_url, models) {
            if capability.status.available() {
                capabilities
                    .entry(capability.model_id.to_ascii_lowercase())
                    .or_default()
                    .push(capability);
            }
        }
        let endpoint_fallback = self
            .endpoint_hint
            .or_else(|| endpoint_url_protocol(base_url))
            .or_else(|| service_protocol(base_url));
        let mut legacy_by_model = BTreeMap::<_, Vec<_>>::new();
        for binding in legacy_bindings {
            let upstream = binding
                .adapter
                .upstream_protocol(binding.wire_api)
                .wire_api();
            for model in &binding.model_ids {
                legacy_by_model
                    .entry(model.to_ascii_lowercase())
                    .or_default()
                    .push(upstream);
            }
        }
        let single_legacy = if legacy_bindings.len() == 1 {
            let binding = &legacy_bindings[0];
            vec![binding
                .adapter
                .upstream_protocol(binding.wire_api)
                .wire_api()]
        } else {
            Vec::new()
        };
        let mut routes = BTreeMap::new();
        let mut seen_models = BTreeSet::new();
        for model in models {
            let key = model.to_ascii_lowercase();
            if !seen_models.insert(key.clone()) {
                continue;
            }
            // Keep old physical endpoints as hints, without retaining their
            // client-protocol restrictions or requiring generation probes.
            let legacy_upstreams = legacy_by_model
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or(&single_legacy);
            let available = capabilities
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_default();
            for client in WireApi::ALL {
                // Resolve each client protocol independently. If the model
                // accepts that protocol upstream, keep it native. Otherwise
                // use the strongest available catalog evidence and bridge to
                // it. If protocol evidence is absent, the configured source
                // protocol is the fallback; catalog membership remains
                // routable regardless of legacy diagnostic probes.
                let upstream = available
                    .iter()
                    .find(|capability| capability.upstream_wire_api == client)
                    .or_else(|| {
                        available.iter().max_by_key(|capability| {
                            (
                                capability.status == CapabilityStatus::Confirmed,
                                capability.checked_at_ms,
                                std::cmp::Reverse(capability.upstream_wire_api),
                            )
                        })
                    })
                    .map(|capability| capability.upstream_wire_api)
                    .unwrap_or_else(|| {
                        endpoint_fallback.unwrap_or_else(|| {
                            legacy_upstreams
                                .iter()
                                .find(|upstream| **upstream == client)
                                .or_else(|| legacy_upstreams.first())
                                .copied()
                                .unwrap_or(fallback)
                        })
                    });
                if let Some(adapter) = SourceAdapter::between(client, upstream) {
                    let model_ids = routes.entry((client, adapter)).or_insert_with(Vec::new);
                    model_ids.push(model.clone());
                }
            }
        }
        Ok(routes
            .into_iter()
            .map(|((wire_api, adapter), model_ids)| SourceProtocolBinding {
                wire_api,
                adapter,
                reasoning_mode: if adapter.is_passthrough() {
                    MessagesReasoningMode::Disabled
                } else {
                    MessagesReasoningMode::Adaptive
                },
                cache_write_ttl: CacheWriteTtl::Provider,
                model_ids,
            })
            .collect())
    }
}

pub fn endpoint_url_protocol(base_url: &str) -> Option<WireApi> {
    let url = url::Url::parse(base_url).ok()?;
    let path = url.path().trim_end_matches('/');
    if path.ends_with("/responses") {
        Some(WireApi::Responses)
    } else if path.ends_with("/chat/completions") {
        Some(WireApi::ChatCompletions)
    } else if path.ends_with("/messages") {
        Some(WireApi::Messages)
    } else if path.ends_with(":generateContent") || path.ends_with(":streamGenerateContent") {
        Some(WireApi::Gemini)
    } else {
        None
    }
}

/// Published service defaults are declarations, never successful probes.
/// Match exact hosts to avoid interpreting lookalike domains as trusted profiles.
pub fn service_protocol(base_url: &str) -> Option<WireApi> {
    let url = url::Url::parse(base_url).ok()?;
    match url.host_str()? {
        "api.openai.com" | "api.zenithmarket.dev" => Some(WireApi::Responses),
        "openrouter.ai" | "api.deepseek.com" | "api.groq.com" | "api.mistral.ai" => {
            Some(WireApi::ChatCompletions)
        }
        "api.anthropic.com" => Some(WireApi::Messages),
        "generativelanguage.googleapis.com" => Some(WireApi::Gemini),
        _ => None,
    }
}

fn endpoint_type(value: &str) -> Option<WireApi> {
    match value {
        "openai-response" | "responses" | "/v1/responses" => Some(WireApi::Responses),
        "openai" | "chat_completions" | "chat.completions" | "/v1/chat/completions" => {
            Some(WireApi::ChatCompletions)
        }
        "anthropic" | "messages" | "/v1/messages" => Some(WireApi::Messages),
        "gemini" | "generateContent" | "streamGenerateContent" => Some(WireApi::Gemini),
        _ => None,
    }
}

pub(crate) fn catalog_capabilities(
    body: &Value,
    checked_at_ms: u64,
) -> Vec<ModelEndpointCapability> {
    let Some(models) = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut capabilities = Vec::new();
    for model in models {
        let Some(id) = model
            .get("id")
            .or_else(|| model.get("name"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let model_id = id.strip_prefix("models/").unwrap_or(id).trim();
        if model_id.is_empty() {
            continue;
        }
        let endpoints = model
            .get("supported_endpoint_types")
            .or_else(|| model.get("supportedEndpointTypes"))
            .or_else(|| model.get("supported_endpoints"))
            .or_else(|| model.get("supportedGenerationMethods"))
            .and_then(Value::as_array);
        let mut protocols = Vec::new();
        for endpoint in endpoints
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(endpoint_type)
        {
            if !protocols.contains(&endpoint) {
                protocols.push(endpoint);
            }
        }
        let advertised_protocols: &[WireApi] = if model.get("supportedGenerationMethods").is_some()
        {
            &[WireApi::Gemini]
        } else {
            &WireApi::ALL
        };
        for &upstream_wire_api in advertised_protocols {
            let status = if endpoints.is_none() {
                CapabilityStatus::Unknown
            } else if protocols.contains(&upstream_wire_api) {
                CapabilityStatus::Declared
            } else {
                CapabilityStatus::Unsupported
            };
            // Participant catalogs identify endpoints, not model capabilities.
            // Semantic fields come from the shared trusted model catalog.
            if endpoints.is_none() {
                continue;
            }
            capabilities.push(ModelEndpointCapability {
                model_id: model_id.to_owned(),
                upstream_wire_api,
                status,
                origin: CapabilityOrigin::Catalog,
                checked_at_ms,
                features: BTreeMap::new(),
                reasoning_efforts: Vec::new(),
            });
        }
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_models_are_routable_without_generation_probes() {
        let config = SourceProtocolConfig::automatic("https://example.test/v1");
        let models = vec!["claude-test".into(), "gpt-test".into()];
        let routes = config
            .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
            .unwrap();
        assert_eq!(routes.len(), 4);
        assert!(routes.iter().all(|route| route.model_ids == models));
        assert!(catalog_capabilities(
            &json!({"data":[{"id":"gpt-test","pricing":{"input":1}}]}),
            1
        )
        .is_empty());
    }

    #[test]
    fn participant_catalog_only_supplies_endpoint_identity() {
        let mut row = json!({"id":"future-model", "supported_reasoning_efforts":["low","high"],
            "capabilities":{"reasoning":true,"tools":false}});
        assert!(catalog_capabilities(&json!({"data":[row.clone()]}), 42).is_empty());
        row["supported_endpoint_types"] = json!(["messages"]);
        let observations = catalog_capabilities(&json!({"data":[row]}), 42);
        assert!(observations
            .iter()
            .all(|entry| entry.features.is_empty() && entry.reasoning_efforts.is_empty()));
        assert!(observations
            .iter()
            .any(|entry| entry.upstream_wire_api == WireApi::Messages && entry.status.available()));
    }

    #[test]
    fn legacy_configuration_uses_automatic_routes() {
        let config = SourceProtocolConfig::default();
        let routes = config
            .resolve(
                "https://example.test/v1",
                &["gpt-test".into()],
                &[],
                WireApi::Responses,
            )
            .unwrap();
        assert_eq!(routes.len(), WireApi::ALL.len());
        assert!(routes.iter().all(|route| route.model_ids == ["gpt-test"]));
    }

    #[test]
    fn legacy_bridge_supplies_physical_protocol_without_restricting_clients() {
        let legacy = [SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            model_ids: vec![],
        }];
        for (url, upstream) in [
            ("https://example.test/v1", WireApi::Messages),
            (
                "https://example.test/v1/chat/completions",
                WireApi::ChatCompletions,
            ),
        ] {
            let routes = SourceProtocolConfig::automatic(url)
                .resolve(url, &["future-model".into()], &legacy, WireApi::Responses)
                .unwrap();
            assert_eq!(routes.len(), WireApi::ALL.len());
            assert!(routes.iter().all(|route| {
                route.model_ids == ["future-model"]
                    && route.adapter.upstream_protocol(route.wire_api).wire_api() == upstream
            }));
        }
    }

    #[test]
    fn endpoint_metadata_and_gemini_methods_are_scoped_declarations() {
        let observations = catalog_capabilities(
            &json!({"data":[
                {"id":"mixed","supported_endpoint_types":["openai","anthropic"]},
                {"id":"unknown"}
            ]}),
            42,
        );
        assert_eq!(observations.len(), 4);
        assert!(observations.iter().all(|entry| entry.model_id == "mixed"));
        assert_eq!(
            observations
                .iter()
                .filter(|entry| entry.status.available())
                .count(),
            2
        );
        let gemini = catalog_capabilities(
            &json!({"models":[
                {"name":"models/test","supportedGenerationMethods":["generateContent","countTokens"]},
                {"name":"models/embed","supportedGenerationMethods":["embedContent"]}
            ]}),
            43,
        );
        assert_eq!(gemini.len(), 2);
        assert_eq!(gemini[0].upstream_wire_api, WireApi::Gemini);
        assert!(!gemini[0]
            .features
            .contains_key(&ProtocolFeature::FunctionTools));
        let mut config =
            SourceProtocolConfig::automatic("https://generativelanguage.googleapis.com/v1beta");
        config.merge_catalog(gemini);
        let routes = config
            .resolve(
                "https://generativelanguage.googleapis.com/v1beta",
                &["test".into(), "embed".into()],
                &[],
                WireApi::Gemini,
            )
            .unwrap();
        assert_eq!(routes.len(), 4);
        assert!(routes
            .iter()
            .all(|route| route.model_ids == ["test", "embed"]));
    }

    #[test]
    fn each_client_protocol_prefers_its_native_upstream() {
        let models = vec!["mixed".into()];
        let mut config = SourceProtocolConfig::default();
        config.merge_catalog(catalog_capabilities(
            &json!({"data":[{
                "id":"mixed",
                "supported_endpoint_types":["responses", "messages"]
            }]}),
            42,
        ));

        let routes = config
            .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
            .unwrap();
        let responses = routes
            .iter()
            .find(|route| route.wire_api == WireApi::Responses)
            .unwrap();
        let messages = routes
            .iter()
            .find(|route| route.wire_api == WireApi::Messages)
            .unwrap();

        assert_eq!(responses.adapter, SourceAdapter::Native);
        assert_eq!(messages.adapter, SourceAdapter::Native);
    }

    #[test]
    fn failed_generation_probe_does_not_remove_catalog_models() {
        let models = vec!["test".into()];
        let mut config = SourceProtocolConfig {
            capabilities: catalog_capabilities(
                &json!({"data":[{"id":"test","supported_endpoint_types":["responses"]}]}),
                1,
            ),
            ..Default::default()
        };
        config.capabilities.push(ModelEndpointCapability {
            model_id: "test".into(),
            upstream_wire_api: WireApi::Responses,
            status: CapabilityStatus::Confirmed,
            origin: CapabilityOrigin::GenerationProbe,
            checked_at_ms: 2,
            features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Unsupported)]),
            reasoning_efforts: vec![],
        });
        let resolved = config
            .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
            .unwrap();
        assert_eq!(resolved.len(), 4);
        assert!(resolved.iter().all(|route| route.model_ids == ["test"]));
        assert!(resolved
            .iter()
            .any(|route| route.wire_api == WireApi::Responses));
        let capabilities = config.effective_capabilities("https://example.test/v1", &models);
        let responses = capabilities
            .iter()
            .find(|entry| entry.upstream_wire_api == WireApi::Responses)
            .unwrap();
        assert_eq!(responses.origin, CapabilityOrigin::Catalog);
        assert_eq!(responses.status, CapabilityStatus::Declared);
        assert_ne!(
            responses.features.get(&ProtocolFeature::Text),
            Some(&CapabilityStatus::Unsupported)
        );
    }

    #[test]
    fn successful_generation_probe_does_not_override_catalog_routes() {
        let models = vec!["test".into()];
        let mut config = SourceProtocolConfig {
            capabilities: catalog_capabilities(
                &json!({"data":[{"id":"test","supported_endpoint_types":["responses"]}]}),
                1,
            ),
            ..Default::default()
        };
        config.capabilities.push(ModelEndpointCapability {
            model_id: "test".into(),
            upstream_wire_api: WireApi::ChatCompletions,
            status: CapabilityStatus::Confirmed,
            origin: CapabilityOrigin::GenerationProbe,
            checked_at_ms: 2,
            features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Confirmed)]),
            reasoning_efforts: vec![],
        });

        let routes = config
            .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
            .unwrap();
        assert!(routes.iter().all(|route| {
            route.adapter.upstream_protocol(route.wire_api).wire_api() == WireApi::Responses
        }));
        assert!(config
            .effective_capabilities("https://example.test/v1", &models)
            .iter()
            .all(|capability| capability.origin != CapabilityOrigin::GenerationProbe));
    }

    #[test]
    fn stored_participant_features_do_not_override_endpoint_identity() {
        let config = SourceProtocolConfig {
            capabilities: vec![ModelEndpointCapability {
                model_id: "synthetic-model".into(),
                upstream_wire_api: WireApi::Messages,
                status: CapabilityStatus::Declared,
                origin: CapabilityOrigin::Catalog,
                checked_at_ms: 1,
                features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Unsupported)]),
                reasoning_efforts: vec![],
            }],
            ..Default::default()
        };
        let routes = config
            .resolve(
                "https://example.test/v1",
                &["synthetic-model".into()],
                &[],
                WireApi::Responses,
            )
            .unwrap();
        assert_eq!(routes.len(), 4);
        assert!(routes.iter().all(|route| route
            .adapter
            .upstream_protocol(route.wire_api)
            .wire_api()
            == WireApi::Messages));
    }

    #[test]
    fn old_configuration_ignores_legacy_mode_and_checks_probe_revision() {
        let mut config: SourceProtocolConfig = serde_json::from_value(json!({})).unwrap();
        let legacy: SourceProtocolConfig =
            serde_json::from_value(json!({"mode":"manual"})).unwrap();
        assert_eq!(legacy, SourceProtocolConfig::default());
        let observation = ModelEndpointCapability {
            model_id: "test".into(),
            upstream_wire_api: WireApi::Responses,
            status: CapabilityStatus::Confirmed,
            origin: CapabilityOrigin::GenerationProbe,
            checked_at_ms: 42,
            features: BTreeMap::new(),
            reasoning_efforts: vec![],
        };
        config.invalidate("https://example.test/v1");
        assert!(!config.apply_probe(0, observation.clone()));
        assert!(config.apply_probe(1, observation));
        config.invalidate("https://other.test/v1");
        assert!(config.capabilities.is_empty());
    }

    #[test]
    fn explicit_endpoint_and_service_profile_do_not_match_lookalikes() {
        assert_eq!(
            endpoint_url_protocol("https://example.test/custom/chat/completions"),
            Some(WireApi::ChatCompletions)
        );
        assert_eq!(
            service_protocol("https://openrouter.ai/api/v1"),
            Some(WireApi::ChatCompletions)
        );
        assert_eq!(
            service_protocol("https://openrouter.ai.example.test/v1"),
            None
        );
        assert_eq!(
            service_protocol("https://api.zenithmarket.dev/v1"),
            Some(WireApi::Responses)
        );
        assert_eq!(
            service_protocol("https://api.zenithmarket.dev.example.test/v1"),
            None
        );
    }

    #[test]
    fn known_responses_service_keeps_unprobed_catalog_models_routable() {
        let models = vec![
            "gpt-5.6-sol".into(),
            "claude-fable-5".into(),
            "gemini-3.8-flash".into(),
            "grok-4.6".into(),
        ];
        let mut config = SourceProtocolConfig::automatic("https://api.zenithmarket.dev/v1");
        config.capabilities.push(ModelEndpointCapability {
            model_id: "gpt-5.6-sol".into(),
            upstream_wire_api: WireApi::Responses,
            status: CapabilityStatus::Confirmed,
            origin: CapabilityOrigin::GenerationProbe,
            checked_at_ms: 42,
            features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Confirmed)]),
            reasoning_efforts: vec![],
        });

        let routes = config
            .resolve(
                "https://api.zenithmarket.dev/v1",
                &models,
                &[],
                WireApi::Responses,
            )
            .unwrap();

        assert_eq!(routes.len(), WireApi::ALL.len());
        for client in WireApi::ALL {
            let route = routes
                .iter()
                .find(|route| route.wire_api == client)
                .expect("every client protocol should have an adapter route");
            assert_eq!(route.model_ids, models);
            assert_eq!(
                route.adapter.upstream_protocol(client).wire_api(),
                WireApi::Responses
            );
        }
    }
}

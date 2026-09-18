use super::{normalize_source_protocol_bindings, SourceProtocolBinding, WireApi};
use crate::{CacheWriteTtl, MessagesReasoningMode, Result, SourceAdapter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Absence in old records is deliberately manual: migration must not invent routes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolSelectionMode {
    Auto,
    #[default]
    Manual,
}

impl ProtocolSelectionMode {
    pub fn is_manual(&self) -> bool {
        *self == Self::Manual
    }
}

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
    #[serde(default)]
    pub mode: ProtocolSelectionMode,
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
        manual_bindings: &[SourceProtocolBinding],
        fallback: WireApi,
    ) -> Vec<ModelEndpointCapability> {
        let hint = self
            .endpoint_hint
            .or_else(|| endpoint_url_protocol(base_url));
        let profile = service_protocol(base_url);
        let manual = if self.mode == ProtocolSelectionMode::Manual {
            normalize_source_protocol_bindings(manual_bindings.to_vec(), fallback, models)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut result = Vec::new();
        for model in models {
            for upstream in WireApi::ALL {
                let mut observations = self
                    .capabilities
                    .iter()
                    .filter(|entry| {
                        entry.model_id.eq_ignore_ascii_case(model)
                            && entry.upstream_wire_api == upstream
                            && entry.status != CapabilityStatus::Unknown
                    })
                    .collect::<Vec<_>>();
                observations.sort_by_key(|entry| {
                    (
                        entry.origin == CapabilityOrigin::GenerationProbe,
                        entry.checked_at_ms,
                    )
                });
                let declaration = if self.mode == ProtocolSelectionMode::Manual {
                    manual
                        .iter()
                        .any(|route| {
                            route.adapter.upstream_protocol(route.wire_api).wire_api() == upstream
                                && route
                                    .model_ids
                                    .iter()
                                    .any(|id| id.eq_ignore_ascii_case(model))
                        })
                        .then_some(CapabilityOrigin::Manual)
                } else if hint == Some(upstream) {
                    Some(CapabilityOrigin::EndpointUrl)
                } else if hint.is_none() && profile == Some(upstream) {
                    Some(CapabilityOrigin::ServiceProfile)
                } else {
                    None
                };
                let mut merged = observations
                    .first()
                    .map(|entry| (*entry).clone())
                    .or_else(|| {
                        declaration.map(|origin| ModelEndpointCapability {
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
                    });
                if let Some(entry) = merged.as_mut() {
                    for observation in observations {
                        entry.status = observation.status;
                        entry.origin = observation.origin;
                        entry.checked_at_ms = observation.checked_at_ms;
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

    pub fn with_effective_capabilities(
        &self,
        base_url: &str,
        models: &[String],
        bindings: &[SourceProtocolBinding],
        fallback: WireApi,
    ) -> Self {
        Self {
            capabilities: self.effective_capabilities(base_url, models, bindings, fallback),
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
        Ok(crate::normalize_model_ids(
            routes
                .into_iter()
                .filter(|route| client.is_none_or(|client| route.wire_api == client))
                .flat_map(|route| route.model_ids)
                .collect::<Vec<_>>(),
        ))
    }

    pub fn automatic(base_url: &str) -> Self {
        Self {
            mode: ProtocolSelectionMode::Auto,
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

    /// Resolve only from protocol evidence. Names, families and prices never
    /// participate. Unknown automatic connections have no generation routes.
    pub fn resolve(
        &self,
        base_url: &str,
        models: &[String],
        manual_bindings: &[SourceProtocolBinding],
        fallback: WireApi,
    ) -> Result<Vec<SourceProtocolBinding>> {
        let capabilities = self.effective_capabilities(base_url, models, manual_bindings, fallback);
        if self.mode == ProtocolSelectionMode::Manual {
            let mut bindings =
                normalize_source_protocol_bindings(manual_bindings.to_vec(), fallback, models)?;
            for binding in &mut bindings {
                let upstream = binding
                    .adapter
                    .upstream_protocol(binding.wire_api)
                    .wire_api();
                binding.model_ids.retain(|model| {
                    !capabilities.iter().any(|capability| {
                        capability.model_id.eq_ignore_ascii_case(model)
                            && capability.upstream_wire_api == upstream
                            && (capability.status == CapabilityStatus::Unsupported
                                || capability.features.get(&ProtocolFeature::Text)
                                    == Some(&CapabilityStatus::Unsupported))
                    })
                });
            }
            bindings.retain(|binding| !binding.model_ids.is_empty());
            return Ok(bindings);
        }
        let mut routes = BTreeMap::new();
        for capability in capabilities.iter().filter(|capability| {
            capability.status.available()
                && capability.features.get(&ProtocolFeature::Text)
                    != Some(&CapabilityStatus::Unsupported)
        }) {
            for client in WireApi::ALL {
                let Some(adapter) = SourceAdapter::between(client, capability.upstream_wire_api)
                else {
                    continue;
                };
                routes
                    .entry((client, adapter))
                    .or_insert_with(Vec::new)
                    .push(capability.model_id.clone());
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
        "api.openai.com" => Some(WireApi::Responses),
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
        let Some(endpoints) = endpoints else { continue };
        let mut protocols = Vec::new();
        for endpoint in endpoints
            .iter()
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
            let status = if protocols.contains(&upstream_wire_api) {
                CapabilityStatus::Declared
            } else {
                CapabilityStatus::Unsupported
            };
            let mut features = BTreeMap::from([(ProtocolFeature::Text, status)]);
            if model
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|method| method.as_str() == Some("streamGenerateContent"))
                })
            {
                features.insert(ProtocolFeature::Streaming, CapabilityStatus::Declared);
            }
            // Providers may advertise explicit feature flags. Missing flags
            // remain unknown; successful catalog retrieval cannot confirm them.
            if let Some(declared) = model.get("capabilities").and_then(Value::as_object) {
                for (key, feature) in [
                    ("vision", ProtocolFeature::Images),
                    ("tools", ProtocolFeature::FunctionTools),
                    ("structured_output", ProtocolFeature::StructuredOutput),
                    ("reasoning", ProtocolFeature::Reasoning),
                    ("streaming", ProtocolFeature::Streaming),
                ] {
                    if let Some(value) = declared.get(key).and_then(Value::as_bool) {
                        features.insert(
                            feature,
                            if value {
                                CapabilityStatus::Declared
                            } else {
                                CapabilityStatus::Unsupported
                            },
                        );
                    }
                }
            }
            let reasoning_efforts = model
                .get("supported_reasoning_efforts")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            capabilities.push(ModelEndpointCapability {
                model_id: model_id.to_owned(),
                upstream_wire_api,
                status,
                origin: CapabilityOrigin::Catalog,
                checked_at_ms,
                features,
                reasoning_efforts,
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
    fn unknown_catalog_does_not_create_generation_routes() {
        let config = SourceProtocolConfig::automatic("https://example.test/v1");
        let models = vec!["claude-test".into(), "gpt-test".into()];
        let routes = config
            .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
            .unwrap();
        assert!(routes.is_empty());
        assert!(catalog_capabilities(
            &json!({"data":[{"id":"gpt-test","pricing":{"input":1}}]}),
            1
        )
        .is_empty());
    }

    #[test]
    fn legacy_manual_catalog_preserves_the_native_configured_wire_api() {
        let config = SourceProtocolConfig::default();
        let routes = config
            .resolve(
                "https://example.test/v1",
                &["gpt-test".into()],
                &[],
                WireApi::Responses,
            )
            .unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].wire_api, WireApi::Responses);
        assert_eq!(routes[0].adapter, SourceAdapter::Native);
        assert_eq!(routes[0].model_ids, ["gpt-test"]);
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
        assert!(routes.iter().all(|route| route.model_ids == ["test"]));
    }

    #[test]
    fn rejected_generation_is_excluded_in_auto_and_manual_without_erasing_other_endpoints() {
        for mode in [ProtocolSelectionMode::Auto, ProtocolSelectionMode::Manual] {
            let models = vec!["test".into()];
            let bindings = [WireApi::Responses, WireApi::Messages]
                .map(|wire| SourceProtocolBinding::legacy(wire, &models));
            let mut config = SourceProtocolConfig {
                mode,
                capabilities: catalog_capabilities(
                    &json!({"data":[{
                        "id":"test", "supported_endpoint_types":["responses", "anthropic"]
                    }]}),
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
                .resolve(
                    "https://example.test/v1",
                    &models,
                    &bindings,
                    WireApi::Responses,
                )
                .unwrap();
            assert!(!resolved.is_empty());
            assert!(resolved.iter().all(|route| route
                .adapter
                .upstream_protocol(route.wire_api)
                .wire_api()
                == WireApi::Messages));
            config.capabilities.last_mut().unwrap().features.clear();
            config.capabilities.last_mut().unwrap().status = CapabilityStatus::Unsupported;
            assert_eq!(
                config
                    .resolve(
                        "https://example.test/v1",
                        &models,
                        &bindings,
                        WireApi::Responses
                    )
                    .unwrap(),
                resolved
            );
            assert_eq!(models, ["test"]);
        }
    }

    #[test]
    fn old_configuration_stays_manual_and_probe_revision_is_checked() {
        let mut config: SourceProtocolConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(config.mode, ProtocolSelectionMode::Manual);
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
    }
}

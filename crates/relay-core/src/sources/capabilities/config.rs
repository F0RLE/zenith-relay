use super::*;

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
        for capability in &self.capabilities {
            // Legacy generation probes are diagnostic records, not routing or
            // model-capability evidence. Never let them override discovery.
            if capability.origin == CapabilityOrigin::GenerationProbe {
                continue;
            }
            if capability.status == CapabilityStatus::Unknown
                && (capability.origin != CapabilityOrigin::Catalog
                    || (capability.features.is_empty() && capability.reasoning_efforts.is_empty()))
            {
                continue;
            }
            indexed
                .entry((
                    crate::model_id_key(&capability.model_id),
                    capability.upstream_wire_api,
                ))
                .or_default()
                .push(capability);
        }
        for observations in indexed.values_mut() {
            observations.sort_by_key(|capability| capability.checked_at_ms);
        }
        let mut effective_capabilities = Vec::new();
        for model in models {
            let model_key = crate::model_id_key(model);
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
                    .or_else(|| observations.first().map(|capability| (*capability).clone()));
                if let Some(effective_capability) = merged.as_mut() {
                    for observation in observations {
                        // Feature-only catalog rows do not establish or erase
                        // the protocol selected by an endpoint declaration.
                        if observation.status != CapabilityStatus::Unknown {
                            effective_capability.status = observation.status;
                            effective_capability.origin = observation.origin;
                            effective_capability.checked_at_ms = observation.checked_at_ms;
                        }
                        effective_capability
                            .features
                            .extend(observation.features.clone());
                        if !observation.reasoning_efforts.is_empty() {
                            effective_capability.reasoning_efforts =
                                observation.reasoning_efforts.clone();
                        }
                    }
                }
                effective_capabilities.extend(merged);
            }
        }
        effective_capabilities
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
            .map(|model| crate::model_id_key(&model))
            .collect::<BTreeSet<_>>();
        Ok(crate::normalize_model_ids(
            models
                .iter()
                .filter(|model| routed_models.contains(&crate::model_id_key(model)))
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
        fallback_protocol: WireApi,
    ) -> Result<Vec<SourceProtocolBinding>> {
        let mut capabilities_by_model = BTreeMap::<_, Vec<_>>::new();
        let mut unsupported_protocols = BTreeMap::<_, BTreeSet<_>>::new();
        for capability in self.effective_capabilities(base_url, models) {
            if capability.status.available() {
                capabilities_by_model
                    .entry(crate::model_id_key(&capability.model_id))
                    .or_default()
                    .push(capability);
            } else if capability.status == CapabilityStatus::Unsupported {
                unsupported_protocols
                    .entry(crate::model_id_key(&capability.model_id))
                    .or_default()
                    .insert(capability.upstream_wire_api);
            }
        }
        let configured_protocol = self
            .endpoint_hint
            .or_else(|| endpoint_url_protocol(base_url))
            .or_else(|| service_protocol(base_url));
        let mut legacy_protocols_by_model = BTreeMap::<_, Vec<_>>::new();
        for binding in legacy_bindings {
            let protocol = binding
                .adapter
                .upstream_protocol(binding.wire_api)
                .wire_api();
            for model in &binding.model_ids {
                legacy_protocols_by_model
                    .entry(crate::model_id_key(model))
                    .or_default()
                    .push(protocol);
            }
        }
        let legacy_fallback_protocols = if legacy_bindings.len() == 1 {
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
            let key = crate::model_id_key(model);
            if !seen_models.insert(key.clone()) {
                continue;
            }
            // Keep old physical endpoints as hints, without retaining their
            // client-protocol restrictions or requiring generation probes.
            let legacy_protocols = legacy_protocols_by_model
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or(&legacy_fallback_protocols);
            let available_capabilities = capabilities_by_model
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_default();
            // Pick one physical upstream contract for the model before
            // projecting any client-facing contract. A model must not reach
            // Responses for one harness and Messages/Gemini for another just
            // because the client wire API changed. The source profile or
            // explicit endpoint hint wins when the catalog does not reject
            // it; otherwise the strongest per-model route evidence wins.
            let selected_protocol = select_upstream_protocol(
                configured_protocol,
                available_capabilities,
                unsupported_protocols.get(&key),
                legacy_protocols,
                fallback_protocol,
            );
            for client_protocol in WireApi::ALL {
                if let Some(adapter) = SourceAdapter::between(client_protocol, selected_protocol) {
                    let model_ids = routes
                        .entry((client_protocol, adapter))
                        .or_insert_with(Vec::new);
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

fn select_upstream_protocol(
    configured_protocol: Option<WireApi>,
    available_capabilities: &[ModelEndpointCapability],
    unsupported_protocols: Option<&BTreeSet<WireApi>>,
    legacy_protocols: &[WireApi],
    fallback_protocol: WireApi,
) -> WireApi {
    if let Some(protocol) = configured_protocol {
        let is_unsupported =
            unsupported_protocols.is_some_and(|protocols| protocols.contains(&protocol));
        if !is_unsupported {
            return protocol;
        }
    }
    available_capabilities
        .iter()
        .max_by_key(|capability| {
            (
                capability.status == CapabilityStatus::Confirmed,
                capability.checked_at_ms,
                std::cmp::Reverse(capability.upstream_wire_api),
            )
        })
        .map(|capability| capability.upstream_wire_api)
        .or_else(|| {
            legacy_protocols
                .iter()
                .find(|protocol| **protocol == fallback_protocol)
                .or_else(|| legacy_protocols.first())
                .copied()
        })
        .or(configured_protocol)
        .unwrap_or(fallback_protocol)
}

/// Field view shared by stored source records.
///
/// Resolution lives here. Each record keeps its own error policy: core
/// `Error`, a display `String`, or an empty fallback are not the same outcome.
pub trait SourceProtocolResolution {
    fn protocol_base_url(&self) -> &str;
    fn protocol_models(&self) -> &[String];
    fn stored_protocol_bindings(&self) -> &[SourceProtocolBinding];
    fn protocol_fallback(&self) -> WireApi;
    fn source_protocol_config(&self) -> &SourceProtocolConfig;

    fn resolved_protocol_bindings(
        &self,
    ) -> std::result::Result<Vec<SourceProtocolBinding>, String> {
        self.source_protocol_config()
            .resolve(
                self.protocol_base_url(),
                self.protocol_models(),
                self.stored_protocol_bindings(),
                self.protocol_fallback(),
            )
            .map_err(|error| error.to_string())
    }

    fn resolved_models(&self, client: Option<WireApi>) -> std::result::Result<Vec<String>, String> {
        self.source_protocol_config()
            .models_for(
                self.protocol_base_url(),
                self.protocol_models(),
                self.stored_protocol_bindings(),
                self.protocol_fallback(),
                client,
            )
            .map_err(|error| error.to_string())
    }

    fn resolved_supports_wire_api(&self, wire_api: WireApi) -> std::result::Result<bool, String> {
        self.resolved_models(Some(wire_api))
            .map(|models| !models.is_empty())
    }

    fn resolved_supports_any(&self) -> std::result::Result<bool, String> {
        self.resolved_models(None).map(|models| !models.is_empty())
    }
}

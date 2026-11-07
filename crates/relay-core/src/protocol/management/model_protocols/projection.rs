use super::{ModelProtocolRoute, SourceSummary};
use crate::model_metadata::ModelCapabilities;
use crate::{
    CapabilityStatus, MessagesReasoningMode, ModelEndpointCapability, ModelTokenLimits,
    SourceAdapter, WireApi,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(super) type EndpointEvidence = Option<Arc<ModelEndpointCapability>>;
type RouteKey = (WireApi, WireApi);
type RouteObservations = BTreeMap<RouteKey, Vec<EndpointEvidence>>;
type SourceEndpoints = BTreeMap<String, BTreeMap<WireApi, Arc<ModelEndpointCapability>>>;

pub(super) fn source_endpoints(source: &SourceSummary) -> SourceEndpoints {
    let mut indexed = SourceEndpoints::new();
    for endpoint in source
        .protocol_config
        .effective_capabilities(&source.base_url, &source.models)
    {
        indexed
            .entry(crate::model_id_key(&endpoint.model_id))
            .or_default()
            .insert(endpoint.upstream_wire_api, Arc::new(endpoint));
    }
    indexed
}

/// Snapshot-local alternatives, not a second runtime registry. Endpoint data is
/// shared across client protocols and never appears in the serialized catalog.
#[derive(Default)]
pub(super) struct EndpointEvidenceIndex {
    observations: BTreeMap<String, RouteObservations>,
}

impl EndpointEvidenceIndex {
    pub(super) fn observe(
        &mut self,
        model: &str,
        client: WireApi,
        upstream: WireApi,
        endpoint: EndpointEvidence,
    ) {
        let alternatives = self
            .observations
            .entry(model.to_owned())
            .or_default()
            .entry((client, upstream))
            .or_default();
        // Multiple unrestricted members have the same reference projection.
        if endpoint.is_none() && alternatives.iter().any(Option::is_none) {
            return;
        }
        alternatives.push(endpoint);
    }

    pub(super) fn project(
        &self,
        model: &str,
        routes: &mut Vec<ModelProtocolRoute>,
        reference: &ModelCapabilities,
        streaming: bool,
    ) {
        let observations = self.observations.get(&crate::model_id_key(model));
        routes.retain_mut(|route| {
            let fallback = [None];
            let alternatives = observations
                .and_then(|routes| routes.get(&(route.client_wire_api, route.upstream_wire_api)))
                .map(Vec::as_slice)
                .unwrap_or(&fallback);
            let mut features = BTreeMap::new();
            let mut efforts = BTreeSet::new();
            let mut ceilings = ModelCapabilities::default();
            let mut eligible = false;
            let adapter = SourceAdapter::between(route.client_wire_api, route.upstream_wire_api);
            for endpoint in alternatives {
                let endpoint = endpoint.as_deref();
                let effective = reference.for_route(endpoint);
                if streaming && effective.streaming == Some(false) {
                    continue;
                }
                eligible = true;
                ModelTokenLimits {
                    context: effective.context_limit,
                    input: effective.input_limit,
                    output: effective.output_limit,
                }
                .constrain(&mut ceilings);
                for (feature, status) in effective.protocol_features_for_route(endpoint) {
                    features
                        .entry(feature)
                        .and_modify(|previous| {
                            *previous = union_status(*previous, status);
                        })
                        .or_insert(status);
                }
                efforts.extend(
                    effective
                        .reasoning_effort_levels
                        .iter()
                        .filter(|effort| {
                            adapter.is_some_and(|adapter| {
                                adapter.supports_reasoning_effort_for_model(
                                    MessagesReasoningMode::Adaptive,
                                    effort,
                                    &effective,
                                )
                            })
                        })
                        .cloned(),
                );
            }
            if !eligible {
                return false;
            }
            route.features = features;
            route.reasoning_efforts = efforts.into_iter().collect();
            route.token_limits = ModelTokenLimits {
                context: ceilings.context_limit,
                input: ceilings.input_limit,
                output: ceilings.output_limit,
            };
            // An empty resolved intersection is not missing reference evidence.
            route.project_reasoning(&[]);
            true
        });
    }
}

fn union_status(left: CapabilityStatus, right: CapabilityStatus) -> CapabilityStatus {
    use CapabilityStatus::{Confirmed, Declared, Unknown, Unsupported};
    match (left, right) {
        (Confirmed, _) | (_, Confirmed) => Confirmed,
        (Declared, _) | (_, Declared) => Declared,
        (Unknown, _) | (_, Unknown) => Unknown,
        (Unsupported, Unsupported) => Unsupported,
    }
}

pub(super) fn capabilities_for_client(
    routes: &[ModelProtocolRoute],
    client: WireApi,
    reference: &ModelCapabilities,
) -> Option<ModelCapabilities> {
    let mut features = BTreeMap::new();
    let mut efforts = Vec::new();
    let mut ceilings = ModelCapabilities::default();
    let mut eligible = false;
    for route in routes.iter().filter(|route| {
        route.client_wire_api == client
            && SourceAdapter::between(client, route.upstream_wire_api).is_some()
            && route.features.get(&crate::ProtocolFeature::Streaming)
                != Some(&CapabilityStatus::Unsupported)
    }) {
        eligible = true;
        for feature in crate::ProtocolFeature::ALL {
            let status = route
                .features
                .get(&feature)
                .copied()
                .unwrap_or(CapabilityStatus::Unknown);
            features
                .entry(feature)
                .and_modify(|previous| *previous = union_status(*previous, status))
                .or_insert(status);
        }
        if route.features.get(&crate::ProtocolFeature::Reasoning)
            != Some(&CapabilityStatus::Unsupported)
        {
            efforts.extend(route.reasoning_efforts.iter().cloned());
        }
        route.token_limits.constrain(&mut ceilings);
    }
    eligible.then(|| {
        reference.for_resolved_route(
            &features,
            &efforts,
            ModelTokenLimits {
                context: ceilings.context_limit,
                input: ceilings.input_limit,
                output: ceilings.output_limit,
            },
        )
    })
}

#[cfg(test)]
mod tests;

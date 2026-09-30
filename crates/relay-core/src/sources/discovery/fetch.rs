use super::{
    catalog_capabilities, detected_model_price, normalized_model_ids, SourceConnector,
    SourceDiscovery, SourceProtocolBinding, SourceProtocolBindingKey, SourceReadHints,
};
use crate::scheduler::refresh::http::{HttpClass, ManagementHttpScope};
use crate::transport::{collect_limited, MAX_MODEL_CATALOG_BODY_BYTES};
use crate::{ApiModelPriceOverride, Error, Result, UpstreamProtocol};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(crate) async fn discover_models_with_client(
    client: &reqwest::Client,
    source: &SourceConnector,
    bindings: &[SourceProtocolBinding],
) -> Result<Vec<String>> {
    // GatewayRuntime only retains normalized bindings, so it cannot tell a
    // legacy expanded model list from an explicit per-protocol allow-list.
    // Keep this compatibility helper broad; management paths use the public
    // discovery API above and retain that distinction.
    discover_protocol_bindings_with_client(
        client,
        source,
        bindings,
        &[],
        &BTreeSet::new(),
        &SourceReadHints::default(),
        &ManagementHttpScope::default(),
    )
    .await
    .map(|discovery| discovery.models)
}

pub(super) async fn discover_protocol_bindings_with_client(
    client: &reqwest::Client,
    source: &SourceConnector,
    bindings: &[SourceProtocolBinding],
    configured_bindings: &[SourceProtocolBinding],
    automatic_catalog_routes: &BTreeSet<SourceProtocolBindingKey>,
    hints: &SourceReadHints,
    scope: &ManagementHttpScope,
) -> Result<SourceDiscovery> {
    let mut last_error = None;
    let mut connector = source.clone();
    let mut resolved_base_url = None;
    let mut discovered_models = Vec::new();
    let mut discovered_model_keys = HashSet::new();
    let mut discovered_bindings = Vec::new();
    let mut detected_model_prices = BTreeMap::new();
    let mut conflicting_model_prices = HashSet::new();
    let mut successful_responses = 0usize;
    let mut capabilities = Vec::new();

    for binding in bindings {
        let (authorization_name, authorization) = source.authorization_for_binding(binding);
        let request = client
            .get(connector.models_url.clone())
            .headers(connector.protocol_headers_for_binding(binding))
            .header(authorization_name.clone(), authorization.clone());
        let (mut response, first_permit) =
            match scope.send(client, request, HttpClass::Ordinary).await {
                Ok(value) => value,
                Err(_) => {
                    last_error = Some(Error::ManagementHttpUnavailable);
                    continue;
                }
            };
        let mut permit = Some(first_permit);
        hints.observe(response.headers());
        if response.status() == reqwest::StatusCode::NOT_FOUND
            && resolved_base_url.is_none()
            && root_v1_fallback_allowed(&connector, binding)
        {
            // The first response has no body to consume. Do not hold its slot
            // while waiting for a separate /v1 retry at the same origin.
            drop(permit.take());
            if let Some(v1_connector) = connector.with_appended_v1(bindings) {
                let retry = client
                    .get(v1_connector.models_url.clone())
                    .headers(v1_connector.protocol_headers_for_binding(binding))
                    .header(authorization_name, authorization);
                if let Ok((candidate, candidate_permit)) =
                    scope.send(client, retry, HttpClass::Ordinary).await
                {
                    hints.observe(candidate.headers());
                    if candidate.status().is_success() {
                        resolved_base_url = Some(
                            v1_connector
                                .base_url
                                .as_str()
                                .trim_end_matches('/')
                                .to_string(),
                        );
                        connector = v1_connector;
                        response = candidate;
                        permit = Some(candidate_permit);
                    }
                }
            }
        }
        if !response.status().is_success() {
            last_error = Some(Error::UpstreamStatus(response.status().as_u16()));
            continue;
        }
        let body = match collect_limited(response, MAX_MODEL_CATALOG_BODY_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        drop(permit);
        let body: Value = match serde_json::from_slice(&body) {
            Ok(body) => body,
            Err(_) => {
                last_error = Some(Error::InvalidUpstreamResponse(
                    "upstream model response is invalid",
                ));
                continue;
            }
        };
        let upstream_models =
            match parse_upstream_models(binding.adapter.upstream_protocol(binding.wire_api), &body)
            {
                Some(models) => models,
                None => {
                    last_error = Some(Error::InvalidUpstreamResponse(
                        "upstream model response is invalid",
                    ));
                    continue;
                }
            };
        successful_responses += 1;
        capabilities.extend(catalog_capabilities(&body, crate::unix_time_ms()));

        // Inventory and price metadata are independent of legacy route lists.
        for (model, price) in &upstream_models {
            let model_key = crate::model_id_key(model);
            if discovered_model_keys.insert(model_key.clone()) {
                discovered_models.push(model.clone());
            }
            if let Some(price) = price {
                record_discovered_model_price(
                    &mut detected_model_prices,
                    &mut conflicting_model_prices,
                    model_key,
                    *price,
                );
            }
        }

        // An explicitly supplied model list is scoped to this protocol unless
        // the route is a source-wide catalog fallback. The normalized legacy
        // binding may contain source.models after expansion, so use the
        // original configured binding to distinguish an allow-list from the
        // legacy "discover everything" form.
        let explicit_models = (!automatic_catalog_routes.contains(&binding.key()))
            .then(|| {
                configured_bindings
                    .iter()
                    .find(|configured| configured.key() == binding.key())
                    .and_then(|configured| {
                        let models = configured
                            .model_ids
                            .iter()
                            .map(|model| crate::model_id_key(model))
                            .filter(|model| !model.is_empty())
                            .collect::<HashSet<_>>();
                        (!models.is_empty()).then_some(models)
                    })
            })
            .flatten();
        let automatic_route_exclusions =
            automatic_catalog_routes.contains(&binding.key()).then(|| {
                bindings
                    .iter()
                    .filter(|candidate| {
                        candidate.wire_api == binding.wire_api && candidate.key() != binding.key()
                    })
                    .flat_map(|candidate| normalized_model_ids(&candidate.model_ids))
                    .collect::<HashSet<_>>()
            });
        let models = upstream_models
            .into_iter()
            .filter(|(model, _)| {
                if automatic_route_exclusions
                    .as_ref()
                    .is_some_and(|excluded| excluded.contains(&crate::model_id_key(model)))
                {
                    return false;
                }
                explicit_models
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&crate::model_id_key(model)))
            })
            .collect::<Vec<_>>();
        discovered_bindings.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            cache_write_ttl: binding.cache_write_ttl,
            model_ids: models.into_iter().map(|(model, _)| model).collect(),
        });
    }

    if successful_responses == 0 {
        return Err(last_error.unwrap_or(Error::InvalidUpstreamResponse(
            "source did not return a valid model catalog",
        )));
    }
    Ok(SourceDiscovery {
        models: discovered_models,
        protocol_bindings: discovered_bindings,
        detected_model_prices,
        resolved_base_url,
        capabilities,
    })
}

fn record_discovered_model_price(
    detected_model_prices: &mut BTreeMap<String, ApiModelPriceOverride>,
    conflicting_model_prices: &mut HashSet<String>,
    model_key: String,
    price: ApiModelPriceOverride,
) {
    if conflicting_model_prices.contains(&model_key) {
        return;
    }
    if let Some(existing) = detected_model_prices.get(&model_key) {
        if existing != &price {
            if let Some(merged) = merge_route_model_price(*existing, price) {
                detected_model_prices.insert(model_key, merged);
            } else {
                detected_model_prices.remove(&model_key);
                conflicting_model_prices.insert(model_key);
            }
        }
        return;
    }
    detected_model_prices.insert(model_key, price);
}

/// A model may be exposed by both a generic OpenAI-compatible route and an
/// Anthropic Messages route in one source. Merge compatible prices while
/// preserving explicit TTL fields from either catalog endpoint.
pub(super) fn merge_route_model_price(
    left: ApiModelPriceOverride,
    right: ApiModelPriceOverride,
) -> Option<ApiModelPriceOverride> {
    if left.input_micro_usd_per_million != right.input_micro_usd_per_million
        || left.cached_input_micro_usd_per_million != right.cached_input_micro_usd_per_million
        || left.output_micro_usd_per_million != right.output_micro_usd_per_million
    {
        return None;
    }
    let cache_write_5m = match (
        left.cache_write_5m_micro_usd_per_million,
        right.cache_write_5m_micro_usd_per_million,
    ) {
        (Some(left), Some(right)) if left != right => return None,
        (Some(value), _) | (_, Some(value)) => Some(value),
        (None, None) => None,
    };
    let cache_write_1h = match (
        left.cache_write_1h_micro_usd_per_million,
        right.cache_write_1h_micro_usd_per_million,
    ) {
        (Some(left), Some(right)) if left != right => return None,
        (Some(value), _) | (_, Some(value)) => Some(value),
        (None, None) => None,
    };
    Some(ApiModelPriceOverride {
        input_micro_usd_per_million: left.input_micro_usd_per_million,
        cached_input_micro_usd_per_million: left.cached_input_micro_usd_per_million,
        cache_write_5m_micro_usd_per_million: cache_write_5m,
        cache_write_1h_micro_usd_per_million: cache_write_1h,
        output_micro_usd_per_million: left.output_micro_usd_per_million,
    })
}

fn root_v1_fallback_allowed(connector: &SourceConnector, binding: &SourceProtocolBinding) -> bool {
    connector.base_url.path() == "/"
        && matches!(
            binding.adapter.upstream_protocol(binding.wire_api),
            UpstreamProtocol::Responses | UpstreamProtocol::ChatCompletions
        )
}

pub(super) fn parse_upstream_models(
    protocol: UpstreamProtocol,
    body: &Value,
) -> Option<Vec<(String, Option<ApiModelPriceOverride>)>> {
    let models = match protocol {
        UpstreamProtocol::GeminiGenerateContent => body
            .get("models")
            .or_else(|| body.get("data"))?
            .as_array()?,
        UpstreamProtocol::Responses
        | UpstreamProtocol::ChatCompletions
        | UpstreamProtocol::Messages => body
            .get("data")
            .or_else(|| body.get("models"))?
            .as_array()?,
    };
    let mut seen = HashSet::new();
    Some(
        models
            .iter()
            .filter_map(|model| {
                let id = match protocol {
                    UpstreamProtocol::GeminiGenerateContent => model
                        .get("supportedGenerationMethods")
                        .and_then(Value::as_array)
                        .filter(|methods| {
                            methods
                                .iter()
                                .any(|method| method.as_str() == Some("generateContent"))
                        })
                        .and_then(|_| model.get("name").or_else(|| model.get("id")))?
                        .as_str()
                        .map(|name| name.strip_prefix("models/").unwrap_or(name)),
                    UpstreamProtocol::Responses
                    | UpstreamProtocol::ChatCompletions
                    | UpstreamProtocol::Messages => model
                        .get("id")
                        .or_else(|| model.get("name"))?
                        .as_str()
                        .map(|name| name.strip_prefix("models/").unwrap_or(name)),
                }?;
                seen.insert(crate::model_id_key(id)).then(|| {
                    (
                        id.to_string(),
                        detected_model_price(model, protocol == UpstreamProtocol::Messages),
                    )
                })
            })
            .collect(),
    )
}

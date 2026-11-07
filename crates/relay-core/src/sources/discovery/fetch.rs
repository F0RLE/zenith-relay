use super::{
    SourceCatalogFailure, SourceConnector, SourceDiscovery, SourceProtocolBinding,
    SourceProtocolBindingKey, SourceReadHints, detected_model_price, normalized_model_ids,
    pages::{CATALOG_TIMEOUT, read_binding_catalog},
};
use crate::scheduler::refresh::http::ManagementHttpScope;
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
    let discovery = discover_protocol_bindings_with_client(
        client,
        source,
        bindings,
        &[],
        &BTreeSet::new(),
        &SourceReadHints::default(),
        &ManagementHttpScope::default(),
    )
    .await?;
    let mut seen = BTreeSet::new();
    let previous = bindings
        .iter()
        .flat_map(|binding| &binding.model_ids)
        .filter(|model| seen.insert(crate::model_id_key(model)))
        .cloned()
        .collect::<Vec<_>>();
    Ok(discovery.models_with_previous(&previous))
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
    let mut reads = Vec::new();
    let mut remaining_budgets = Vec::<std::time::Duration>::new();
    for (index, binding) in bindings.iter().enumerate() {
        let started = tokio::time::Instant::now();
        let mut result =
            read_binding_catalog(client, &connector, binding, bindings, hints, scope).await;
        let remaining = CATALOG_TIMEOUT.saturating_sub(started.elapsed());
        if let Ok(catalog) = &mut result
            && let Some(resolved) = catalog.resolved_connector.take()
        {
            resolved_base_url = Some(resolved.base_url.as_str().trim_end_matches('/').to_string());
            connector = resolved;
            // A source has one base URL. Re-read earlier catalogs at the
            // resolved root instead of mixing evidence from different paths.
            for ((previous, read), &budget) in bindings[..index]
                .iter()
                .zip(&mut reads)
                .zip(&remaining_budgets)
            {
                *read = if budget.is_zero() {
                    Err(Error::ManagementHttpUnavailable)
                } else {
                    tokio::time::timeout(
                        budget,
                        read_binding_catalog(client, &connector, previous, bindings, hints, scope),
                    )
                    .await
                    .unwrap_or(Err(Error::ManagementHttpUnavailable))
                };
            }
        }
        reads.push(result);
        remaining_budgets.push(remaining);
    }
    if resolved_base_url.is_some() && reads.iter().any(Result::is_err) {
        return Err(reads
            .into_iter()
            .rev()
            .find_map(Result::err)
            .unwrap_or(Error::ManagementHttpUnavailable));
    }
    let partial = reads.iter().any(Result::is_err);
    let mut failures = Vec::new();
    let mut catalog_models = BTreeMap::<_, Vec<String>>::new();
    for (binding, read) in bindings.iter().zip(reads) {
        let catalog = match read {
            Ok(catalog) => catalog,
            Err(error) => {
                failures.push(SourceCatalogFailure::new(binding, &error));
                last_error = Some(error);
                continue;
            }
        };
        let upstream_models = catalog.models;
        if partial {
            catalog_models
                .entry(
                    binding
                        .adapter
                        .upstream_protocol(binding.wire_api)
                        .wire_api(),
                )
                .or_default()
                .extend(upstream_models.iter().map(|(model, _)| model.clone()));
        }
        successful_responses += 1;
        capabilities.extend(catalog.capabilities);

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
        let mut binding_models = HashSet::new();
        discovered_bindings.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            cache_write_ttl: binding.cache_write_ttl,
            model_ids: models
                .into_iter()
                .map(|(model, _)| model)
                .filter(|model| binding_models.insert(crate::model_id_key(model)))
                .collect(),
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
        catalog_models,
        failures,
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
        || (!left.flex.is_empty() && !right.flex.is_empty() && left.flex != right.flex)
        || (!left.priority.is_empty()
            && !right.priority.is_empty()
            && left.priority != right.priority)
    {
        return None;
    }
    let short_cache_write_price = match (
        left.cache_write_5m_micro_usd_per_million,
        right.cache_write_5m_micro_usd_per_million,
    ) {
        (Some(left), Some(right)) if left != right => return None,
        (Some(price), _) | (_, Some(price)) => Some(price),
        (None, None) => None,
    };
    let long_cache_write_price = match (
        left.cache_write_1h_micro_usd_per_million,
        right.cache_write_1h_micro_usd_per_million,
    ) {
        (Some(left), Some(right)) if left != right => return None,
        (Some(price), _) | (_, Some(price)) => Some(price),
        (None, None) => None,
    };
    let generic_cache_write_price = match (
        left.cache_write_micro_usd_per_million,
        right.cache_write_micro_usd_per_million,
    ) {
        (Some(left), Some(right)) if left != right => return None,
        (Some(price), _) | (_, Some(price)) => Some(price),
        (None, None) => None,
    };
    let mut merged = ApiModelPriceOverride {
        cache_write_micro_usd_per_million: generic_cache_write_price,
        cache_write_5m_micro_usd_per_million: short_cache_write_price,
        cache_write_1h_micro_usd_per_million: long_cache_write_price,
        flex: if left.flex.is_empty() {
            right.flex
        } else {
            left.flex
        },
        priority: if left.priority.is_empty() {
            right.priority
        } else {
            left.priority
        },
        ..left
    };
    for (target, incoming) in [
        (&mut merged.above_100k, right.above_100k),
        (&mut merged.above_128k, right.above_128k),
        (&mut merged.above_200k, right.above_200k),
        (&mut merged.above_272k, right.above_272k),
        (&mut merged.above_512k, right.above_512k),
    ] {
        if target.is_empty() {
            *target = incoming;
        } else if !incoming.is_empty() && *target != incoming {
            return None;
        }
    }
    Some(merged)
}

pub(super) fn parse_upstream_models(
    protocol: UpstreamProtocol,
    catalog_payload: &Value,
) -> Option<Vec<(String, Option<ApiModelPriceOverride>)>> {
    let model_records: &[Value] = match protocol {
        UpstreamProtocol::GeminiGenerateContent | UpstreamProtocol::GeminiInteractions => {
            match catalog_payload
                .get("models")
                .or_else(|| catalog_payload.get("data"))
            {
                Some(records) => records.as_array()?,
                None if catalog_payload
                    .as_object()?
                    .keys()
                    .all(|key| key == "nextPageToken") =>
                {
                    &[]
                }
                None => return None,
            }
        }
        UpstreamProtocol::Responses
        | UpstreamProtocol::ChatCompletions
        | UpstreamProtocol::Messages => catalog_payload
            .get("data")
            .or_else(|| catalog_payload.get("models"))?
            .as_array()?,
    };
    let mut seen_model_ids = HashSet::new();
    let mut models = Vec::new();
    for model_record in model_records {
        let raw_model_id = match protocol {
            UpstreamProtocol::GeminiGenerateContent | UpstreamProtocol::GeminiInteractions => {
                model_record.get("name").or_else(|| model_record.get("id"))
            }
            UpstreamProtocol::Responses
            | UpstreamProtocol::ChatCompletions
            | UpstreamProtocol::Messages => {
                model_record.get("id").or_else(|| model_record.get("name"))
            }
        }?
        .as_str()?
        .trim();
        let model_id = raw_model_id
            .strip_prefix("models/")
            .unwrap_or(raw_model_id)
            .trim();
        // A malformed row makes the snapshot incomplete. It must not turn
        // a failed refresh into a valid empty or partial inventory.
        if model_id.is_empty() {
            return None;
        }
        if protocol == UpstreamProtocol::GeminiGenerateContent
            && !model_record
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|method| method.as_str() == Some("generateContent"))
                })
        {
            continue;
        }
        if seen_model_ids.insert(crate::model_id_key(model_id)) {
            models.push((model_id.to_string(), detected_model_price(model_record)));
        }
    }
    Some(models)
}

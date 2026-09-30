use super::{CacheWriteTtl, WireApi};
use crate::{Error, MessagesReasoningMode, Result, SourceAdapter, UpstreamProtocol};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
/// Associates a client-facing wire contract, an explicit adapter, and the
/// models that are known to work through that route.
///
/// `Native` keeps the client and upstream contracts equal. A bridge changes the
/// upstream contract only when it is explicitly selected and validated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProtocolBinding {
    pub wire_api: WireApi,
    #[serde(default)]
    pub adapter: SourceAdapter,
    #[serde(default)]
    pub reasoning_mode: MessagesReasoningMode,
    #[serde(default)]
    #[serde(skip_serializing_if = "CacheWriteTtl::is_provider")]
    pub cache_write_ttl: CacheWriteTtl,
    #[serde(default)]
    pub model_ids: Vec<String>,
}

/// Stable in-memory identity for one source connector route.
///
/// A source can expose more than one Responses-facing route when the models
/// behind those routes require different upstream contracts. The adapter is
/// part of the identity: `responses/native` and
/// `responses/responses_to_messages` are distinct routes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceProtocolBindingKey {
    pub wire_api: WireApi,
    pub adapter: SourceAdapter,
}

impl SourceProtocolBinding {
    pub fn legacy(wire_api: WireApi, models: &[String]) -> Self {
        Self {
            wire_api,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            model_ids: models.to_vec(),
        }
    }

    pub const fn key(&self) -> SourceProtocolBindingKey {
        SourceProtocolBindingKey {
            wire_api: self.wire_api,
            adapter: self.adapter,
        }
    }

    /// Whether this route can carry a Responses reasoning effort to its
    /// upstream contract. Native routes preserve provider-defined values;
    /// bridges are limited to their explicit translation mode.
    pub fn supports_reasoning_effort(&self, effort: &str) -> bool {
        self.adapter
            .supports_reasoning_effort(self.reasoning_mode, effort)
    }
}

/// Normalizes source protocol bindings while keeping the source-provided
/// model order.
///
/// A legacy source has one implicit binding and therefore still expands an
/// empty model list to the source catalog. For an explicitly mixed source an
/// empty list means that the route has not been verified yet. Expanding it to
/// every source model would make an unknown route look usable, so the binding
/// stays empty until discovery fills it.
pub fn normalize_source_protocol_bindings(
    bindings: Vec<SourceProtocolBinding>,
    fallback_wire_api: WireApi,
    models: &[String],
) -> Result<Vec<SourceProtocolBinding>> {
    let models = crate::catalog::normalize_model_ids(models);
    let known_models = models
        .iter()
        .map(|model| crate::model_id_key(model))
        .collect::<HashSet<_>>();
    let bindings = if bindings.is_empty() {
        vec![SourceProtocolBinding::legacy(fallback_wire_api, &models)]
    } else {
        bindings
    };
    let mut seen_routes = BTreeSet::new();
    let mut normalized = Vec::with_capacity(bindings.len());

    let expand_empty_models = bindings.len() == 1 && bindings[0].adapter.is_passthrough();
    for binding in bindings {
        // Source bindings select only an upstream protocol adapter. Reasoning
        // availability belongs to the pool's model rule; the Messages bridge
        // always uses its single technical translation path. Ignore legacy
        // source-level values while normalizing persisted records. Both
        // bridge adapters have an explicit local translation policy; native
        // routes stay opaque and therefore do not advertise a synthetic mode.
        let reasoning_mode = if !binding.adapter.is_passthrough() {
            MessagesReasoningMode::Adaptive
        } else {
            MessagesReasoningMode::Disabled
        };
        binding
            .adapter
            .validate(binding.wire_api, reasoning_mode)
            .map_err(|error| {
                Error::Validation(format!(
                    "source protocol binding is invalid: {}",
                    error.message()
                ))
            })?;
        if binding.cache_write_ttl != CacheWriteTtl::Provider
            && binding.adapter.upstream_protocol(binding.wire_api) != UpstreamProtocol::Messages
        {
            return Err(Error::Validation(
                "cache write TTL requires a Messages upstream route".to_string(),
            ));
        }
        if !seen_routes.insert(binding.key()) {
            return Err(Error::Validation(
                "each client protocol and adapter route may be configured only once".to_string(),
            ));
        }
        let mut model_ids = crate::catalog::normalize_model_ids(binding.model_ids);
        if model_ids.is_empty() && expand_empty_models {
            model_ids = models.clone();
        }
        if !known_models.is_empty()
            && model_ids
                .iter()
                .any(|model| !known_models.contains(&crate::model_id_key(model)))
        {
            return Err(Error::Validation(
                "source protocol binding references a model not exposed by the source".to_string(),
            ));
        }
        normalized.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode,
            cache_write_ttl: binding.cache_write_ttl,
            model_ids,
        });
    }

    Ok(normalized)
}

/// Returns the explicitly configured source protocol bindings for runtime use.
///
/// Adapters are deliberate routing decisions, not an inferred fallback. In
/// particular, a native Messages binding must not silently create a
/// Responses-to-Messages route: the latter changes the request contract and
/// must be selected and persisted explicitly by the operator.
pub fn runtime_source_protocol_bindings(
    bindings: Vec<SourceProtocolBinding>,
    fallback_wire_api: WireApi,
    models: &[String],
) -> Result<Vec<SourceProtocolBinding>> {
    normalize_source_protocol_bindings(bindings, fallback_wire_api, models)
}

/// Returns the normalized source models available through one client protocol.
pub fn source_models_for_wire_api(
    protocol_bindings: &[SourceProtocolBinding],
    fallback_wire_api: WireApi,
    source_models: &[String],
    wire_api: WireApi,
) -> Result<Vec<String>> {
    Ok(normalize_source_protocol_bindings(
        protocol_bindings.to_vec(),
        fallback_wire_api,
        source_models,
    )?
    .into_iter()
    .filter(|binding| binding.wire_api == wire_api)
    .flat_map(|binding| binding.model_ids)
    .collect())
}

/// Returns the models reachable through one client protocol after applying
/// Relay's runtime links between confirmed native and adapted routes.
pub fn runtime_source_models_for_wire_api(
    protocol_bindings: &[SourceProtocolBinding],
    fallback_wire_api: WireApi,
    source_models: &[String],
    wire_api: WireApi,
) -> Result<Vec<String>> {
    Ok(runtime_source_protocol_bindings(
        protocol_bindings.to_vec(),
        fallback_wire_api,
        source_models,
    )?
    .into_iter()
    .filter(|binding| binding.wire_api == wire_api)
    .flat_map(|binding| binding.model_ids)
    .collect())
}

/// Returns the deduplicated source catalog reachable through every confirmed
/// client protocol. The output keeps the public protocol order and then the
/// provider-provided order within each route.
pub fn runtime_source_models_for_any_wire_api(
    protocol_bindings: &[SourceProtocolBinding],
    fallback_wire_api: WireApi,
    source_models: &[String],
) -> Result<Vec<String>> {
    let bindings = runtime_source_protocol_bindings(
        protocol_bindings.to_vec(),
        fallback_wire_api,
        source_models,
    )?;
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for wire_api in WireApi::ALL {
        for binding in bindings
            .iter()
            .filter(|binding| binding.wire_api == wire_api)
        {
            for model in &binding.model_ids {
                if seen.insert(crate::model_id_key(model)) {
                    models.push(model.clone());
                }
            }
        }
    }
    Ok(models)
}

/// Returns models with a confirmed Anthropic-style Messages upstream route.
/// Cache creation pricing is valid only for these model/route combinations.
pub fn runtime_source_models_with_cache_write_pricing(
    protocol_bindings: &[SourceProtocolBinding],
    fallback_wire_api: WireApi,
    source_models: &[String],
) -> BTreeSet<String> {
    cache_write_model_ids(
        runtime_source_protocol_bindings(
            protocol_bindings.to_vec(),
            fallback_wire_api,
            source_models,
        )
        .unwrap_or_default(),
    )
}

/// Models whose resolved route speaks Anthropic Messages upstream.
pub fn cache_write_model_ids(
    bindings: impl IntoIterator<Item = SourceProtocolBinding>,
) -> BTreeSet<String> {
    bindings
        .into_iter()
        .filter(|binding| {
            binding.adapter.upstream_protocol(binding.wire_api) == UpstreamProtocol::Messages
        })
        .flat_map(|binding| binding.model_ids)
        .map(|model| crate::model_id_key(&model))
        .collect()
}

/// Reports whether a confirmed route exposes at least one model through the
/// requested client protocol.
pub fn runtime_source_supports_wire_api(
    protocol_bindings: &[SourceProtocolBinding],
    fallback_wire_api: WireApi,
    source_models: &[String],
    wire_api: WireApi,
) -> Result<bool> {
    Ok(!runtime_source_models_for_wire_api(
        protocol_bindings,
        fallback_wire_api,
        source_models,
        wire_api,
    )?
    .is_empty())
}

/// Reports whether a confirmed route exposes at least one model through any
/// supported client protocol.
pub fn runtime_source_supports_any_wire_api(
    protocol_bindings: &[SourceProtocolBinding],
    fallback_wire_api: WireApi,
    source_models: &[String],
) -> Result<bool> {
    Ok(!runtime_source_models_for_any_wire_api(
        protocol_bindings,
        fallback_wire_api,
        source_models,
    )?
    .is_empty())
}

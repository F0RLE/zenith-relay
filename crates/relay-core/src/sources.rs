mod capabilities;
mod connector;
mod discovery;
mod observations;
mod probe;
mod services;
pub use observations::SourceRead;
mod stats;

pub use probe::{
    SourceProbeInput, SourceProbeResult, probe_source_generation,
    probe_source_generation_with_scope,
};

pub use capabilities::{
    CapabilityOrigin, CapabilityStatus, ModelEndpointCapability, ModelTokenLimits, ProtocolFeature,
    SourceProtocolConfig, SourceProtocolResolution, endpoint_url_protocol, service_protocol,
};
pub use connector::SourceConnector;
pub(crate) use discovery::discover_models_with_client;
pub use discovery::{
    SourceCatalogEvidence, SourceCatalogRecord, SourceDiscovery, discover_source_models,
    discover_source_models_and_protocol_bindings, discover_source_models_for_protocol_bindings,
    discover_source_with_protocol_config, discover_source_with_protocol_config_with_scope,
    read_source_models, read_source_models_with_scope, source_catalog_changed,
};
pub use stats::{
    SourceBalanceKind, SourceProviderStats, SourceStatsAmount, SourceStatsCurrency,
    SourceStatsProvider, SourceStatsStatus, fetch_source_provider_stats,
    fetch_source_provider_stats_with_proxy, read_source_provider_stats,
    read_source_provider_stats_with_proxy, read_source_provider_stats_with_scope,
    read_source_provider_stats_with_scope_and_proxy,
};
#[cfg(test)]
use stats::{openrouter_stats, source_stats_endpoint, source_stats_provider, zenith_stats};

#[cfg(test)]
use crate::MessagesReasoningMode;
use crate::SourceAdapter;

use std::collections::BTreeMap;

mod binding;
mod provider;
mod wire;

pub use binding::{
    SourceProtocolBinding, SourceProtocolBindingKey, cache_write_model_ids,
    normalize_source_protocol_bindings, runtime_source_models_for_any_wire_api,
    runtime_source_models_for_wire_api, runtime_source_models_with_cache_write_pricing,
    runtime_source_protocol_bindings, runtime_source_supports_any_wire_api,
    runtime_source_supports_wire_api, source_models_for_wire_api,
};
pub use provider::{
    LocalGatewayKey, ProviderSource, is_http_endpoint, is_loopback_url, source_points_to_gateway,
    url_has_userinfo,
};
pub(crate) use provider::{normalized_base_url, redact_url};
pub use wire::{CacheWriteTtl, WireApi};

/// Longest operator-configured wait before a source route is eligible again.
pub const MAX_SOURCE_RECOVERY_DELAY_SECONDS: u64 = 24 * 60 * 60;

/// Stored fields that select a source runtime.
/// Priority, weight, membership, and recovery delay can change without it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceTransportIdentity<'a> {
    pub id: &'a str,
    pub base_url: &'a str,
    pub secret_ref: &'a str,
    pub proxy_id: Option<&'a str>,
    pub wire_api: WireApi,
    pub protocol_bindings: &'a [SourceProtocolBinding],
    pub protocol_config: &'a SourceProtocolConfig,
    pub models: &'a [String],
}

/// Host records expose the stored fields that select a source runtime.
pub trait SourceTransportRecord {
    fn transport_identity(&self) -> SourceTransportIdentity<'_>;
}

impl SourceTransportRecord for SourceTransportIdentity<'_> {
    fn transport_identity(&self) -> SourceTransportIdentity<'_> {
        *self
    }
}

/// A hot policy update is safe only when every source keeps the same runtime.
pub fn source_runtime_policy_compatible<T: SourceTransportRecord>(
    previous_sources: &[T],
    updated_sources: &[T],
) -> bool {
    if previous_sources.len() != updated_sources.len() {
        return false;
    }
    // A keyed snapshot avoids quadratic rescanning on large inventories and
    // refuses ambiguous duplicate identities instead of matching one twice.
    let mut updated = BTreeMap::new();
    for source in updated_sources {
        let identity = source.transport_identity();
        if updated.insert(identity.id, identity).is_some() {
            return false;
        }
    }
    previous_sources.iter().all(|source| {
        let identity = source.transport_identity();
        updated
            .remove(identity.id)
            .is_some_and(|candidate| candidate == identity)
    })
}

/// Writes operator priorities onto existing sources.
/// The error is the source id that was not in the list.
pub fn apply_source_priorities<T>(
    sources: &mut [T],
    priorities: &BTreeMap<String, i32>,
    source_id: impl Fn(&T) -> &str,
    set_priority: impl Fn(&mut T, i32),
) -> std::result::Result<(), String> {
    for (id, priority) in priorities {
        let Some(source) = sources.iter_mut().find(|source| source_id(source) == id) else {
            return Err(id.clone());
        };
        set_priority(source, *priority);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

use super::*;
use zenith_relay_core::{PoolParticipant, SourceTransportRecord};

/// Keeps an API source editable after an automatic catalog request fails.
/// Its routes remain configured, but are intentionally model-less until a
/// later successful refresh confirms upstream capabilities.
pub(super) fn empty_source_discovery(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
) -> LocalResult<SourceDiscovery> {
    let protocol_bindings = normalize_source_protocol_bindings(
        protocol_bindings
            .iter()
            .cloned()
            .map(|mut binding| {
                binding.model_ids.clear();
                binding
            })
            .collect(),
        source.wire_api,
        &[],
    )
    .map_err(LocalPoolError::invalid_state)?;
    Ok(SourceDiscovery {
        models: Vec::new(),
        protocol_bindings,
        resolved_base_url: None,
        detected_model_prices: BTreeMap::new(),
        capabilities: Vec::new(),
    })
}

pub(super) fn source_probe_matches(
    before: &ProviderSourceRecord,
    updated_source: &ProviderSourceRecord,
) -> bool {
    before.transport_identity() == updated_source.transport_identity()
}

pub(super) fn source_dispatch_configuration_changed(
    previous_source: &ProviderSourceRecord,
    updated_source: &ProviderSourceRecord,
) -> bool {
    !source_probe_matches(previous_source, updated_source)
        || zenith_relay_core::pool_dispatch_permission_changed(
            previous_source.pool_access(),
            updated_source.pool_access(),
        )
}

pub(super) fn source_runtime_policy_compatible(
    previous_sources: &[ProviderSourceRecord],
    updated_sources: &[ProviderSourceRecord],
) -> bool {
    zenith_relay_core::source_runtime_policy_compatible(previous_sources, updated_sources)
}

pub(super) fn source_catalog_visibility_changed(
    previous_sources: &[ProviderSourceRecord],
    updated_sources: &[ProviderSourceRecord],
) -> bool {
    previous_sources.iter().any(|source| {
        let Some(candidate) = updated_sources
            .iter()
            .find(|candidate| candidate.id == source.id)
        else {
            return source.in_pool;
        };
        zenith_relay_core::pool_catalog_visibility_changed(
            source.pool_access(),
            candidate.pool_access(),
        )
    })
}

pub(crate) fn validate_source_record(
    state: &DesktopState,
    source: &ProviderSourceRecord,
) -> LocalResult<()> {
    if source.recovery_delay_seconds > zenith_relay_core::MAX_SOURCE_RECOVERY_DELAY_SECONDS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source recovery delay must not exceed 24 hours",
        ));
    }
    source
        .validate_price_overrides()
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    source
        .validate_protocol_bindings()
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error))?;
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    let runtime_source = ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key,
        wire_api: source.wire_api,
        models: source.models.clone(),
    };
    runtime_source.validate().map_err(core_error)?;
    ensure_not_gateway_self_source(state, &runtime_source.base_url)
}

pub(super) fn detected_prices_for_upstream(
    source: &ProviderSourceRecord,
    base_url: &str,
    wire_api: &WireApi,
) -> BTreeMap<String, ApiModelPriceOverride> {
    if source.base_url == base_url.trim() && &source.wire_api == wire_api {
        source.detected_model_prices.clone()
    } else {
        BTreeMap::new()
    }
}

pub(in crate::local_pool) fn ensure_not_gateway_self_source(
    state: &DesktopState,
    base_url: &str,
) -> LocalResult<()> {
    let gateway = state.store()?.gateway().clone();
    let gateway_base_url = format!("http://{}:{}/v1", gateway.client_host, gateway.port);
    if source_points_to_gateway(base_url, &gateway_base_url) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
}

pub(super) fn current_records(
    state: &DesktopState,
) -> LocalResult<(
    Vec<ProviderSourceRecord>,
    Vec<crate::local_pool::models::LocalGatewayKeyRecord>,
)> {
    let store = state.store()?;
    Ok((store.sources().to_vec(), store.keys().to_vec()))
}

pub(super) fn responses_wire_api() -> WireApi {
    WireApi::Responses
}

pub(super) fn default_weight() -> u32 {
    1
}

pub(super) fn normalize_pricing_identity(
    pricing_identity: Option<String>,
) -> LocalResult<Option<String>> {
    zenith_relay_core::normalize_pricing_identity(pricing_identity).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "pricing identity contains unsupported characters",
        )
    })
}

pub(super) fn apply_source_priorities(
    sources: &mut [ProviderSourceRecord],
    priorities: &BTreeMap<String, i32>,
) -> LocalResult<()> {
    zenith_relay_core::apply_source_priorities(
        sources,
        priorities,
        |source| source.id.as_str(),
        |source, priority| source.priority = priority,
    )
    .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "source priority target not found"))
}

use super::*;

pub(super) fn source_record(
    id: String,
    secret_ref: String,
    input: SourceInput,
) -> Result<SourceRecord, ManagementError> {
    let protocol_config = SourceProtocolConfig::automatic(&input.base_url);
    let mut source_record = SourceRecord {
        id,
        name: clean_label(&input.name, "source name")?,
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: input.base_url.trim().to_string(),
        secret_ref,
        pricing_provider: normalize_pricing_identity(input.pricing_provider, "pricing provider")?,
        official_provider_family: normalize_pricing_identity(
            input.official_provider_family,
            "official provider family",
        )?,
        wire_api: input.wire_api,
        protocol_bindings: input.protocol_bindings,
        protocol_config,
        models: normalized_values(input.models),
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        recovery_delay_seconds: valid_recovery_delay(input.recovery_delay_seconds)?,
        model_price_overrides: normalize_source_prices(input.model_price_overrides)?,
        detected_model_prices: BTreeMap::new(),
        last_error_code: None,
    };
    normalize_record_protocol_bindings(&mut source_record)?;
    validate_source_record(&source_record, &input.api_key)?;
    Ok(source_record)
}

pub(super) fn validate_source_record(
    source_record: &SourceRecord,
    api_key: &str,
) -> Result<(), ManagementError> {
    normalize_pricing_identity(source_record.pricing_provider.clone(), "pricing provider")?;
    normalize_pricing_identity(
        source_record.official_provider_family.clone(),
        "official provider family",
    )?;
    valid_recovery_delay(source_record.recovery_delay_seconds)?;
    normalize_source_prices(source_record.model_price_overrides.clone())?;
    normalize_source_prices(source_record.detected_model_prices.clone())?;
    validate_record_protocol_bindings(source_record)?;
    ProviderSource {
        id: source_record.id.clone(),
        name: source_record.name.clone(),
        base_url: source_record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: source_record.wire_api,
        models: source_record.models.clone(),
    }
    .validate()
    .map_err(|error| validation_error(error.to_string()))
}

pub(super) fn normalize_pricing_identity(
    pricing_identity: Option<String>,
    label: &str,
) -> Result<Option<String>, ManagementError> {
    zenith_relay_core::normalize_pricing_identity(pricing_identity).map_err(|_| {
        ManagementError::validation(
            error_codes::SOURCE_PRICING_IDENTITY_INVALID,
            format!("{label} contains unsupported characters"),
        )
    })
}

fn validate_record_protocol_bindings(source_record: &SourceRecord) -> Result<(), ManagementError> {
    if source_record.protocol_bindings.is_empty() {
        return Ok(());
    }
    source_record
        .effective_protocol_bindings()
        .map(drop)
        .map_err(|error| validation_error(error.to_string()))
}

pub(super) fn normalize_record_protocol_bindings(
    source_record: &mut SourceRecord,
) -> Result<(), ManagementError> {
    source_record
        .effective_protocol_bindings()
        .map(drop)
        .map_err(validation_error)
}

fn clear_source_binding_models(bindings: &mut [SourceProtocolBinding]) {
    for binding in bindings {
        binding.model_ids.clear();
    }
}

pub(super) fn clear_source_catalog(source_record: &mut SourceRecord) {
    source_record.models.clear();
    source_record.detected_model_prices.clear();
    clear_source_binding_models(&mut source_record.protocol_bindings);
}

pub(super) fn valid_recovery_delay(delay_seconds: u64) -> Result<u64, ManagementError> {
    (delay_seconds <= zenith_relay_core::MAX_SOURCE_RECOVERY_DELAY_SECONDS)
        .then_some(delay_seconds)
        .ok_or_else(|| {
            ManagementError::validation(
                error_codes::SOURCE_RECOVERY_DELAY_INVALID,
                "source recovery delay must not exceed 24 hours",
            )
        })
}

pub(super) fn normalize_source_prices(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, ManagementError> {
    normalize_model_price_overrides(prices).map_err(|message| {
        ManagementError::validation(error_codes::SOURCE_MODEL_PRICE_INVALID, message)
    })
}

pub(super) fn find_source(
    state: &AppState,
    source_id: &str,
) -> Result<SourceRecord, ManagementError> {
    state
        .store
        .sources()
        .map_err(store_error)?
        .into_iter()
        .find(|source_record| source_record.id == source_id)
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_NOT_FOUND, "source not found")
        })
}

pub(super) fn source_summary(
    state: &AppState,
    source_record: &SourceRecord,
) -> Result<SourceSummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .sources
        .into_iter()
        .find(|source_summary| source_summary.id == source_record.id)
        .ok_or_else(|| {
            ManagementError::internal(error_codes::SNAPSHOT_MISSING, "source snapshot missing")
        })
}

pub(super) fn ensure_not_server_self_source(
    state: &AppState,
    source_base_url: &str,
) -> Result<(), ManagementError> {
    let gateway_base_url = format!(
        "{}/v1",
        state.config.public_base_url.as_str().trim_end_matches('/')
    );
    if source_points_to_gateway(source_base_url, &gateway_base_url) {
        return Err(ManagementError::validation(
            error_codes::SOURCE_SELF_ROUTE,
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
}

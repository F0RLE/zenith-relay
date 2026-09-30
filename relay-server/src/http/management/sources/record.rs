use super::*;

pub(super) fn source_record(
    id: String,
    secret_ref: String,
    input: SourceInput,
) -> Result<SourceRecord, ManagementError> {
    let protocol_config = SourceProtocolConfig::automatic(&input.base_url);
    let mut record = SourceRecord {
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
    normalize_record_protocol_bindings(&mut record)?;
    validate_source_record(&record, &input.api_key)?;
    Ok(record)
}

pub(super) fn validate_source_record(
    record: &SourceRecord,
    api_key: &str,
) -> Result<(), ManagementError> {
    normalize_pricing_identity(record.pricing_provider.clone(), "pricing provider")?;
    normalize_pricing_identity(
        record.official_provider_family.clone(),
        "official provider family",
    )?;
    valid_recovery_delay(record.recovery_delay_seconds)?;
    normalize_source_prices(record.model_price_overrides.clone())?;
    normalize_source_prices(record.detected_model_prices.clone())?;
    validate_record_protocol_bindings(record)?;
    ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    }
    .validate()
    .map_err(|error| validation_error(error.to_string()))
}

pub(super) fn normalize_pricing_identity(
    value: Option<String>,
    label: &str,
) -> Result<Option<String>, ManagementError> {
    zenith_relay_core::normalize_pricing_identity(value).map_err(|_| {
        ManagementError::validation(
            error_codes::SOURCE_PRICING_IDENTITY_INVALID,
            format!("{label} contains unsupported characters"),
        )
    })
}

fn validate_record_protocol_bindings(record: &SourceRecord) -> Result<(), ManagementError> {
    if record.protocol_bindings.is_empty() {
        return Ok(());
    }
    record
        .effective_protocol_bindings()
        .map(drop)
        .map_err(|error| validation_error(error.to_string()))
}

pub(super) fn normalize_record_protocol_bindings(
    record: &mut SourceRecord,
) -> Result<(), ManagementError> {
    record
        .effective_protocol_bindings()
        .map(drop)
        .map_err(validation_error)
}

fn clear_source_binding_models(bindings: &mut [SourceProtocolBinding]) {
    for binding in bindings {
        binding.model_ids.clear();
    }
}

pub(super) fn clear_source_catalog(record: &mut SourceRecord) {
    record.models.clear();
    record.detected_model_prices.clear();
    clear_source_binding_models(&mut record.protocol_bindings);
}

pub(super) fn valid_recovery_delay(value: u64) -> Result<u64, ManagementError> {
    (value <= zenith_relay_core::MAX_SOURCE_RECOVERY_DELAY_SECONDS)
        .then_some(value)
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

pub(super) fn find_source(state: &AppState, id: &str) -> Result<SourceRecord, ManagementError> {
    state
        .store
        .sources()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_NOT_FOUND, "source not found")
        })
}

pub(super) fn source_summary(
    state: &AppState,
    record: &SourceRecord,
) -> Result<SourceSummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .sources
        .into_iter()
        .find(|value| value.id == record.id)
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

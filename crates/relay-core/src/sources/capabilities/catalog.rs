use super::endpoint_type;
use super::*;

pub(crate) fn catalog_capabilities(
    catalog_response: &Value,
    checked_at_ms: u64,
) -> Vec<ModelEndpointCapability> {
    let Some(models) = catalog_response
        .get("data")
        .or_else(|| catalog_response.get("models"))
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
        let mut protocols = Vec::new();
        for endpoint in endpoints
            .into_iter()
            .flatten()
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
        let declared_service_tiers = model
            .get("service_tiers")
            .and_then(Value::as_array)
            .or_else(|| {
                model
                    .get("supported_service_tiers")
                    .and_then(Value::as_array)
            })
            .map(|_| crate::catalog::reference_service_tiers(model));
        let token_limits = ModelTokenLimits::from_catalog(model);
        for &upstream_wire_api in advertised_protocols {
            let status = if endpoints.is_none() {
                CapabilityStatus::Unknown
            } else if protocols.contains(&upstream_wire_api) {
                CapabilityStatus::Declared
            } else {
                CapabilityStatus::Unsupported
            };
            // Endpoint-local numeric limits may restrict this route. Other
            // semantic fields still come from the trusted reference catalog.
            if endpoints.is_none() && declared_service_tiers.is_none() && token_limits.is_empty() {
                continue;
            }
            capabilities.push(ModelEndpointCapability {
                model_id: model_id.to_owned(),
                upstream_wire_api,
                status,
                origin: CapabilityOrigin::Catalog,
                checked_at_ms,
                features: BTreeMap::new(),
                reasoning_efforts: Vec::new(),
                token_limits,
                service_tiers: declared_service_tiers.clone(),
            });
        }
    }
    capabilities
}

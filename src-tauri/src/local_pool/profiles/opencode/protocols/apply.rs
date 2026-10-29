use super::*;

pub(in crate::local_pool::profiles::opencode) fn apply(
    config: &mut Map<String, Value>,
    base: &str,
    secret: &str,
    models: &[ModelSummary],
) -> Result<(), LocalPoolError> {
    let mut groups = BTreeMap::<WireApi, Vec<ModelSummary>>::new();
    for model in models
        .iter()
        .filter(|model| model.enabled && !model.protocol_routes.is_empty())
    {
        // Keep the old provider/model identifier while it is still a usable
        // route, but never let a converted route pin a model that has a native
        // one: the group's own protocol keeps provider semantics and billing.
        let has_native = WireApi::ALL
            .into_iter()
            .any(|protocol| native(model, protocol));
        let previous_protocol = existing_protocol(config, &model.id, |protocol| {
            if has_native {
                native(model, protocol)
            } else {
                supports(model, protocol)
            }
        });
        groups
            .entry(previous_protocol.unwrap_or_else(|| preferred(model)))
            .or_default()
            .push(model.clone());
    }
    let groups = GROUPS
        .iter()
        .map(|(protocol, _, _)| {
            provider(
                base,
                secret,
                &groups.remove(protocol).unwrap_or_default(),
                *protocol,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    merge_groups(config, groups, false)
}

pub(in crate::local_pool::profiles::opencode) fn apply_source(
    config: &mut Map<String, Value>,
    source: &ProviderSourceRecord,
    secret: &str,
    metadata: &ModelMetadataCatalog,
    select_connection: bool,
) -> Result<(), LocalPoolError> {
    let routes =
        zenith_relay_core::SourceProtocolResolution::resolved_protocol_bindings_with_catalog(
            source,
            Some(metadata),
        )
        .map_err(LocalPoolError::invalid_state)?;
    let mut groups = BTreeMap::<WireApi, Vec<String>>::new();
    let native = routes
        .into_iter()
        .filter(|binding| binding.adapter == SourceAdapter::Native)
        .collect::<Vec<_>>();
    let model_ids = zenith_relay_core::normalize_model_ids(
        native.iter().flat_map(|binding| binding.model_ids.iter()),
    );
    for model in model_ids {
        let supported = native
            .iter()
            .filter(|binding| binding.model_ids.contains(&model))
            .map(|binding| binding.wire_api)
            .collect::<Vec<_>>();
        let protocol = metadata
            .native_protocol_for(&model)
            .or_else(|| metadata.preferred_protocol_for(&model))
            .filter(|protocol| supported.contains(protocol))
            .or_else(|| {
                supported
                    .contains(&source.wire_api)
                    .then_some(source.wire_api)
            })
            .or_else(|| supported.first().copied())
            .ok_or_else(|| {
                LocalPoolError::invalid_state("source model has no native protocol route")
            })?;
        groups.entry(protocol).or_default().push(model);
    }
    let mut generated_groups = Vec::new();
    for (protocol, _, _) in GROUPS {
        let models = groups.remove(&protocol).unwrap_or_default();
        let mut configured = model_config_ids(&models, metadata);
        // Direct connections cannot execute Relay translations. The SDK must
        // speak the exact native protocol declared by this source.
        for provider_model_config in configured.values_mut() {
            set_variants(provider_model_config, protocol);
        }
        let generated = provider_with_models(&source.base_url, secret, configured, protocol)?;
        generated_groups.push(generated);
    }
    merge_groups(config, generated_groups, select_connection)
}

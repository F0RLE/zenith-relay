use super::*;
use crate::WireApi;

pub(super) fn build_sources(
    sources: Vec<RuntimeSource>,
    registry: &mut ModelRegistry,
    scheduler: &mut PoolScheduler,
    reference_catalog: Option<&crate::model_metadata::ModelMetadataCatalog>,
) -> Result<SourceRuntimeParts> {
    let mut executors = BTreeMap::new();
    let mut candidate_bindings = BTreeMap::new();
    let mut recovery_delays_ms = BTreeMap::new();
    let mut legacy_candidate_ids = BTreeMap::<String, Option<String>>::new();
    for source in sources {
        source.source.validate()?;
        if source.weight == 0 {
            return Err(Error::Validation(
                "source weight must be at least one".to_string(),
            ));
        }
        if source.recovery_delay_seconds > crate::MAX_SOURCE_RECOVERY_DELAY_SECONDS {
            return Err(Error::Validation(
                "source recovery delay must not exceed 24 hours".to_string(),
            ));
        }
        if executors.contains_key(&source.source.id) {
            return Err(Error::Validation("source ids must be unique".to_string()));
        }
        let bindings = source.protocol_config.resolve_with_catalog(
            &source.source.base_url,
            &source.source.models,
            &source.protocol_bindings,
            source.source.wire_api,
            reference_catalog,
        )?;
        let source_id = source.source.id.clone();
        let connector = SourceConnector::new(&source.source, &bindings)?;
        let discovered_capabilities = source
            .protocol_config
            .effective_capabilities(&source.source.base_url, &source.source.models);
        let rules = model_rules(&source.allowed_models, &source.excluded_models);
        let mut routes = Vec::new();
        let mut model_protocols = BTreeMap::<String, BTreeSet<WireApi>>::new();
        for binding in &bindings {
            let models = normalized_set(binding.model_ids.iter());
            if models.is_empty() {
                continue;
            }
            let legacy_id = source_candidate_id(&source_id, binding, source.source.wire_api);
            legacy_candidate_ids
                .entry(legacy_id)
                .and_modify(|owner| {
                    if owner.as_ref() != Some(&source_id) {
                        *owner = None;
                    }
                })
                .or_insert_with(|| Some(source_id.clone()));
            for model in &models {
                model_protocols
                    .entry(crate::model_id_key(model))
                    .or_default()
                    .insert(binding.wire_api);
            }
            routes.push(SourceCandidateRoute {
                binding_key: binding.key(),
                wire_api: binding.wire_api,
                adapter: binding.adapter,
                reasoning_mode: binding.reasoning_mode,
                cache_write_ttl: binding.cache_write_ttl,
                models: models
                    .iter()
                    .map(|model| crate::model_id_key(model))
                    .collect(),
                capabilities: discovered_capabilities
                    .iter()
                    .filter(|capability| {
                        capability.upstream_wire_api
                            == binding
                                .adapter
                                .upstream_protocol(binding.wire_api)
                                .wire_api()
                            && binding.model_ids.iter().any(|model| {
                                crate::model_id_key(model)
                                    == crate::model_id_key(&capability.model_id)
                            })
                    })
                    .map(|capability| {
                        (
                            crate::model_id_key(&capability.model_id),
                            capability.clone(),
                        )
                    })
                    .collect(),
            });
        }
        if !routes.is_empty() {
            let executable_models = normalized_set(
                source
                    .source
                    .models
                    .iter()
                    .filter(|model| model_protocols.contains_key(&crate::model_id_key(model))),
            );
            let candidate = RuntimeCandidate {
                id: source_id.clone(),
                kind: CandidateKind::ApiSource,
                source_id: source_id.clone(),
                account_id: None,
                protocol: source.source.wire_api,
                models: executable_models,
                model_protocols,
                enabled: source.enabled,
                draining: source.draining,
                priority: source.priority,
                weight: source.weight,
                model_rules: rules.clone(),
                health: CandidateHealth::Healthy,
                quota: CandidateQuota::Unknown,
                provider_credits_micro_units: None,
                provider_credits_unlimited: false,
                provider_credits_observed_at_ms: None,
                quota_updated_at_ms: None,
                quota_reset_at_ms: None,
                cooldowns: BTreeMap::new(),
                last_used_at: source.last_used_at_ms,

                secret_available: true,
            };
            registry.replace(source_id.clone(), source.source.models.iter());
            scheduler.upsert(candidate);
            if source.recovery_delay_seconds > 0 {
                recovery_delays_ms.insert(
                    source_id.clone(),
                    source.recovery_delay_seconds.saturating_mul(1_000),
                );
            }
            candidate_bindings.insert(
                source_id.clone(),
                SourceCandidateBinding {
                    source_id: source_id.clone(),
                    routes,
                },
            );
        }
        executors.insert(source_id, connector);
    }
    Ok(SourceRuntimeParts {
        executors,
        candidate_bindings,
        recovery_delays_ms,
        legacy_candidate_ids,
    })
}

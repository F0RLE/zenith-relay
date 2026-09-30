use super::*;

pub(super) fn build_sources(
    sources: Vec<RuntimeSource>,
    registry: &mut ModelRegistry,
    scheduler: &mut PoolScheduler,
) -> Result<SourceRuntimeParts> {
    let mut executors = BTreeMap::new();
    let mut candidate_bindings = BTreeMap::new();
    let mut recovery_delays_ms = BTreeMap::new();
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
        let bindings = source.protocol_config.resolve(
            &source.source.base_url,
            &source.source.models,
            &source.protocol_bindings,
            source.source.wire_api,
        )?;
        let source_id = source.source.id.clone();
        let connector = SourceConnector::new(&source.source, &bindings)?;
        let rules = model_rules(&source.allowed_models, &source.excluded_models);
        for binding in &bindings {
            let models = normalized_set(binding.model_ids.iter());
            if models.is_empty() {
                continue;
            }
            let candidate_id = source_candidate_id(&source_id, binding, source.source.wire_api);
            if candidate_bindings.contains_key(&candidate_id) {
                return Err(Error::Validation(
                    "source protocol candidate ids must be unique".to_string(),
                ));
            }
            let candidate = RuntimeCandidate {
                id: candidate_id.clone(),
                kind: CandidateKind::ApiSource,
                source_id: source_id.clone(),
                account_id: None,
                protocol: binding.wire_api,
                enabled: source.enabled,
                draining: source.draining,
                priority: source.priority,
                weight: source.weight,
                models: models.clone(),
                model_rules: rules.clone(),
                health: CandidateHealth::Healthy,
                quota: CandidateQuota::Unknown,
                provider_credits_micro_units: None,
                provider_credits_unlimited: false,
                quota_updated_at_ms: None,
                quota_reset_at_ms: None,
                cooldowns: BTreeMap::new(),
                last_used_at: source.last_used_at_ms,

                secret_available: true,
            };
            registry.replace(candidate_id.clone(), source.source.models.iter());
            scheduler.upsert(candidate);
            scheduler.set_native_route(&candidate_id, binding.adapter.is_passthrough());
            if source.recovery_delay_seconds > 0 {
                recovery_delays_ms.insert(
                    candidate_id.clone(),
                    source.recovery_delay_seconds.saturating_mul(1_000),
                );
            }
            candidate_bindings.insert(
                candidate_id,
                SourceCandidateBinding {
                    source_id: source_id.clone(),
                    binding_key: binding.key(),
                    wire_api: binding.wire_api,
                    adapter: binding.adapter,
                    reasoning_mode: binding.reasoning_mode,
                    cache_write_ttl: binding.cache_write_ttl,
                },
            );
        }
        executors.insert(source_id, connector);
    }
    Ok(SourceRuntimeParts {
        executors,
        candidate_bindings,
        recovery_delays_ms,
    })
}

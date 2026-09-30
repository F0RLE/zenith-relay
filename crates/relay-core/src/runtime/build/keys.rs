use super::*;

pub(super) fn build_keys(keys: Vec<RuntimeMixedLocalKey>) -> Result<KeyRuntimeParts> {
    let mut runtime_keys = Vec::new();
    let mut configured_rules = Vec::new();
    let mut key_ids = HashSet::new();
    for key in keys {
        key.key.validate()?;
        if !key_ids.insert(key.key.id.clone()) {
            return Err(Error::Validation(
                "gateway credential ids must be unique".to_string(),
            ));
        }
        let scope = CandidateScope {
            source_ids: key.source_ids.map(|ids| normalized_set(ids.iter())),
            account_ids: key.account_ids.map(|ids| normalized_set(ids.iter())),
            model_rules: ModelRules::default(),
        };
        let base_model_rules = ModelRules {
            allowed: normalized_set(key.allowed_models.iter()),
            excluded: normalized_set(key.excluded_models.iter()),
        };
        let client_wire_apis = key.wire_apis.map(|values| {
            values
                .into_iter()
                .map(normalize_client_wire_api)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        });
        if client_wire_apis.as_ref().is_some_and(Vec::is_empty) {
            return Err(Error::Validation(
                "gateway credential protocol scope must not be empty".to_string(),
            ));
        }
        configured_rules.push(ConfiguredKeyRule {
            enabled: key.enabled,
            scope: scope.clone(),
            model_rules: base_model_rules.clone(),
            client_wire_apis: client_wire_apis.clone(),
        });
        runtime_keys.push(RuntimeKey {
            id: key.key.id,
            enabled: key.enabled,
            secret_hash: Sha256::digest(key.key.secret.as_bytes()).into(),
            scope: Arc::new(RwLock::new(scope)),
            scope_revision: Arc::new(AtomicU64::new(0)),
            model_rules: base_model_rules,
            model_prefix: normalize_prefix(key.model_prefix),
            client_wire_apis,
        });
    }
    Ok(KeyRuntimeParts {
        runtime_keys,
        configured_rules,
    })
}

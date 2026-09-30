use super::*;

fn owns_connection(config: &Map<String, Value>, base: &str, secret: &str) -> bool {
    let Some(providers) = config.get("provider").and_then(Value::as_object) else {
        return false;
    };
    let mut found = false;
    for (protocol, id, npm) in protocols::GROUPS {
        let Some(provider) = providers.get(id) else {
            continue;
        };
        found = true;
        let Ok(base) = protocols::base_url(base, protocol) else {
            return false;
        };
        if provider.get("npm").and_then(Value::as_str) != Some(npm)
            || provider.pointer("/options/baseURL").and_then(Value::as_str) != Some(&base)
            || provider.pointer("/options/apiKey").and_then(Value::as_str) != Some(secret)
        {
            return false;
        }
    }
    found
}

pub(in crate::local_pool) async fn refresh_active_opencode_catalog(
    state: &DesktopState,
) -> Result<(), LocalPoolError> {
    let path = default_opencode_config_path();
    if !path.exists() || (!backup_path(state).exists() && !missing_marker_path(state).exists()) {
        return Ok(());
    }
    let original = fs::read(&path).map_err(|_| {
        LocalPoolError::new(ErrorCode::Io, "OpenCode configuration could not be read")
    })?;
    let mut config = read_config(&path)?;
    if !config
        .get("provider")
        .and_then(Value::as_object)
        .is_some_and(|providers| providers.keys().any(|id| protocols::managed_id(id)))
    {
        return Ok(());
    }
    let snapshot = super::super::state::build_local_runtime_state(state)
        .await
        .map_err(|error| LocalPoolError::new(error.code, error.message))?;
    let (key, sources) = {
        let store = state.store()?;
        (
            store.keys().iter().find(|key| key.system).cloned(),
            store.sources().to_vec(),
        )
    };
    let mut updated = false;
    if let Some(key) = key {
        if let Some(secret) = secret_store::load(&key.secret_ref)? {
            if owns_connection(&config, &snapshot.gateway.base_url, &secret) {
                protocols::apply(
                    &mut config,
                    &snapshot.gateway.base_url,
                    &secret,
                    &snapshot.gateway.models,
                )?;
                updated = true;
            }
        }
    }
    if !updated {
        for source in sources.iter().filter(|source| source.enabled) {
            let Some(secret) = secret_store::load(&source.secret_ref)? else {
                continue;
            };
            if owns_connection(&config, &source.base_url, &secret) {
                protocols::apply_source(
                    &mut config,
                    source,
                    &secret,
                    &state.model_metadata_catalog(),
                    false,
                )?;
                updated = true;
                break;
            }
        }
    }
    // A newer login or external edit owns the file once its endpoint or key changes.
    if updated
        && fs::read(&path).ok().as_deref() == Some(original.as_slice())
        && read_config(&path)? != config
    {
        write_config(&path, &config)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_requires_every_managed_group_to_keep_its_endpoint_and_credential() {
        let mut config = Map::new();
        protocols::apply(&mut config, "http://127.0.0.1:14998/v1", "synthetic", &[]).unwrap();
        assert!(owns_connection(
            &config,
            "http://127.0.0.1:14998/v1",
            "synthetic"
        ));
        config.get_mut("provider").unwrap()["zenith-relay"]["options"]["apiKey"] = "changed".into();
        assert!(!owns_connection(
            &config,
            "http://127.0.0.1:14998/v1",
            "synthetic"
        ));
        assert!(!owns_connection(
            &config,
            "http://127.0.0.1:14999/v1",
            "changed"
        ));
    }
}

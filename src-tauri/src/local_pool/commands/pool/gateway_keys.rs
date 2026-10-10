use super::super::cleanup_created_secret;
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalGatewayKeyRecord, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use chrono::Utc;
use std::collections::BTreeSet;
use uuid::Uuid;

const SYSTEM_GATEWAY_KEY_LABEL: &str = "ChatGPT pool";
pub(in crate::local_pool) const SYSTEM_GATEWAY_KEY_ID: &str = "key_system";

pub(in crate::local_pool::commands) fn new_local_gateway_api_key() -> String {
    format!("zlr_{}", Uuid::new_v4().simple())
}

pub(in crate::local_pool::commands) fn ensure_local_gateway_key_secret(
    key: &LocalGatewayKeyRecord,
) -> LocalResult<String> {
    if let Some(secret) = secret_store::load(&key.secret_ref)? {
        return Ok(secret);
    }
    let secret = new_local_gateway_api_key();
    secret_store::save(&key.secret_ref, &secret)?;
    Ok(secret)
}

pub(in crate::local_pool::commands) fn ensure_system_gateway_key(
    state: &DesktopState,
) -> LocalResult<LocalGatewayKeyRecord> {
    let existing = {
        let keys = state.store()?.keys().to_vec();
        keys.iter()
            .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
            .or_else(|| keys.iter().find(|key| key.system))
            .cloned()
    };
    if let Some(mut key) = existing {
        if !key.enabled || !key.system || key.label != SYSTEM_GATEWAY_KEY_LABEL {
            key.enabled = true;
            key.system = true;
            key.label = SYSTEM_GATEWAY_KEY_LABEL.into();
            state.store()?.upsert_key(key.clone())?;
        }
        ensure_local_gateway_key_secret(&key)?;
        return Ok(key);
    }

    let system_key_id = SYSTEM_GATEWAY_KEY_ID.to_string();
    let key = LocalGatewayKeyRecord {
        secret_ref: system_gateway_secret_ref(&system_key_id),
        id: system_key_id,
        label: SYSTEM_GATEWAY_KEY_LABEL.into(),
        enabled: true,
        system: true,
        created_at: Utc::now().to_rfc3339(),
        last_used_at: None,
    };
    ensure_local_gateway_key_secret(&key)?;
    if let Err(error) = state.store()?.upsert_key(key.clone()) {
        cleanup_created_secret(&key.secret_ref, &error)?;
        return Err(error);
    }
    Ok(key)
}

fn system_gateway_secret_ref(system_key_id: &str) -> String {
    #[cfg(test)]
    {
        format!("key:{system_key_id}:test_{}", Uuid::new_v4().simple())
    }
    #[cfg(not(test))]
    {
        format!("key:{system_key_id}")
    }
}

pub(crate) fn retire_user_gateway_keys(state: &DesktopState) -> LocalResult<()> {
    let (sources, all_keys, canonical_id) = {
        let store = state.store()?;
        let keys = store.keys().to_vec();
        let canonical_id = keys
            .iter()
            .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
            .or_else(|| keys.iter().find(|key| key.system))
            .map(|key| key.id.clone());
        (store.sources().to_vec(), keys, canonical_id)
    };
    let retired = all_keys
        .iter()
        .filter(|key| canonical_id.as_deref() != Some(key.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut retained = all_keys
        .iter()
        .filter(|key| canonical_id.as_deref() == Some(key.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let needs_normalization = retained
        .first()
        .is_some_and(|key| !key.enabled || !key.system || key.label != SYSTEM_GATEWAY_KEY_LABEL);
    if let Some(key) = retained.first_mut() {
        key.enabled = true;
        key.system = true;
        key.label = SYSTEM_GATEWAY_KEY_LABEL.into();
    }
    if retired.is_empty() && !needs_normalization {
        return Ok(());
    }
    let retained_secret_ref = retained.first().map(|key| key.secret_ref.as_str());
    let retired_secrets = load_retired_gateway_secrets(&retired, retained_secret_ref)?;
    state.store()?.replace_records(sources.clone(), retained)?;

    let mut attempted_refs = Vec::new();
    for (secret_ref, _) in &retired_secrets {
        attempted_refs.push(secret_ref.clone());
        if let Err(error) = secret_store::delete(secret_ref) {
            return Err(rollback_gateway_key_cleanup(
                state,
                sources,
                all_keys,
                &retired_secrets,
                &attempted_refs,
                error,
            ));
        }
    }
    Ok(())
}

fn load_retired_gateway_secrets(
    retired: &[LocalGatewayKeyRecord],
    retained_secret_ref: Option<&str>,
) -> LocalResult<Vec<(String, Option<String>)>> {
    let mut secret_refs = BTreeSet::new();
    let mut secrets = Vec::new();
    for key in retired {
        if retained_secret_ref == Some(key.secret_ref.as_str())
            || !secret_refs.insert(key.secret_ref.clone())
        {
            continue;
        }
        secrets.push((key.secret_ref.clone(), secret_store::load(&key.secret_ref)?));
    }
    Ok(secrets)
}

fn rollback_gateway_key_cleanup(
    state: &DesktopState,
    sources: Vec<ProviderSourceRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
    retired_secrets: &[(String, Option<String>)],
    attempted_refs: &[String],
    cause: LocalPoolError,
) -> LocalPoolError {
    let mut failures = Vec::new();
    let records_result = match state.store() {
        Ok(mut store) => store.replace_records(sources, old_keys),
        Err(error) => Err(error),
    };
    if let Err(error) = records_result {
        failures.push(format!("state restore failed: {error}"));
    }
    for (secret_ref, secret) in retired_secrets {
        if !attempted_refs
            .iter()
            .any(|attempted_secret_ref| attempted_secret_ref == secret_ref)
        {
            continue;
        }
        if let Some(secret) = secret {
            if let Err(error) = secret_store::save(secret_ref, secret) {
                failures.push(format!("secret restore failed: {error}"));
            }
        }
    }
    if failures.is_empty() {
        cause
    } else {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; legacy gateway credential cleanup rollback failed: {}",
                cause.message,
                failures.join("; ")
            ),
        )
    }
}

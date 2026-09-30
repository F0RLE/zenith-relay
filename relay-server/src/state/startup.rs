use super::{
    now_ms, AccountCredential, GatewayKeyRecord, ServerProxyRecord, COMMON_PROXY_SECRET_REF,
    PROFILE_KEY_ROTATION_PREFIX, SYSTEM_GATEWAY_KEY_ID,
};
use crate::store::{Store, Vault};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub fn ensure_proxy_record(
    store: &Store,
    vault: &Vault,
    value: &str,
) -> Result<ServerProxyRecord, String> {
    let value = zenith_relay_core::normalize_proxy_url(value)?;
    let id = proxy_id(&value);
    if let Some(record) = store.proxy(&id)? {
        match vault.load(&record.secret_ref)? {
            Some(stored) if stored != value => {
                return Err("stored proxy reference is inconsistent".to_string())
            }
            Some(_) => {}
            None => vault.save(&record.secret_ref, &value)?,
        }
        return Ok(record);
    }
    let url = url::Url::parse(&value).map_err(|_| "stored proxy URL is invalid".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "stored proxy URL is invalid".to_string())?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let record = ServerProxyRecord {
        id: id.clone(),
        endpoint: format!(
            "{}://{}:{}",
            url.scheme(),
            host,
            url.port_or_known_default().unwrap_or_default()
        ),
        secret_ref: format!("proxy:{id}"),
        created_at_ms: now_ms(),
    };
    vault.save(&record.secret_ref, &value)?;
    if let Err(error) = store.save_proxy(&record) {
        let _ = vault.delete(&record.secret_ref);
        return Err(error);
    }
    Ok(record)
}

pub(super) fn migrate_legacy_proxies(store: &Store, vault: &Vault) -> Result<(), String> {
    if store.common_proxy_id()?.is_none() && store.common_proxy_configured()? {
        if let Some(value) = vault.load(COMMON_PROXY_SECRET_REF)? {
            let proxy = ensure_proxy_record(store, vault, &value)?;
            store.set_common_proxy_id(Some(&proxy.id))?;
        }
    }
    for mut record in store.accounts()? {
        let Some(secret) = vault.load(&record.secret_ref)? else {
            continue;
        };
        let mut credential: AccountCredential = serde_json::from_str(&secret)
            .map_err(|_| "stored account credential is invalid".to_string())?;
        if record.proxy_id.is_none() {
            if let Some(value) = credential.proxy_url.as_deref() {
                record.proxy_id = Some(ensure_proxy_record(store, vault, value)?.id);
                store.save_account(&record)?;
            }
        }
        if record.proxy_id.is_some() && credential.proxy_url.take().is_some() {
            vault.save(
                &record.secret_ref,
                &serde_json::to_string(&credential)
                    .map_err(|_| "stored account credential is invalid".to_string())?,
            )?;
        }
    }
    Ok(())
}

pub(super) fn retire_user_gateway_keys(store: &Store, vault: &Vault) -> Result<(), String> {
    let keys = store.keys()?;
    let retained_secret_refs = keys
        .iter()
        .filter(|key| key.id == SYSTEM_GATEWAY_KEY_ID || is_internal_gateway_key(key))
        .map(|key| key.secret_ref.clone())
        .collect::<HashSet<_>>();
    let retired = keys
        .into_iter()
        .filter(|key| {
            key.id != SYSTEM_GATEWAY_KEY_ID
                && !is_internal_gateway_key(key)
                && !retained_secret_refs.contains(&key.secret_ref)
        })
        .collect::<Vec<_>>();
    if retired.is_empty() {
        return Ok(());
    }

    let mut secret_refs = HashSet::new();
    let mut secrets = Vec::new();
    for key in &retired {
        if !secret_refs.insert(key.secret_ref.clone()) {
            continue;
        }
        secrets.push((key.secret_ref.clone(), vault.load(&key.secret_ref)?));
    }
    let ids = retired.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
    let mut attempted_refs = Vec::new();
    for (secret_ref, _) in &secrets {
        attempted_refs.push(secret_ref.clone());
        if let Err(error) = vault.delete(secret_ref) {
            return Err(rollback_retired_gateway_keys(
                vault,
                &secrets,
                &attempted_refs,
                format!("legacy gateway credential cleanup failed: {error}"),
            ));
        }
    }
    if let Err(error) = store.delete_keys(&ids) {
        return Err(rollback_retired_gateway_keys(
            vault,
            &secrets,
            &attempted_refs,
            format!("legacy gateway credential records could not be removed: {error}"),
        ));
    }
    Ok(())
}

fn rollback_retired_gateway_keys(
    vault: &Vault,
    secrets: &[(String, Option<String>)],
    attempted_refs: &[String],
    cause: String,
) -> String {
    let mut failures = Vec::new();
    for (secret_ref, secret) in secrets {
        if !attempted_refs.iter().any(|value| value == secret_ref) {
            continue;
        }
        if let Some(secret) = secret {
            if let Err(error) = vault.save(secret_ref, secret) {
                failures.push(format!("secret restore failed: {error}"));
            }
        }
    }
    if failures.is_empty() {
        cause
    } else {
        format!("{cause}; cleanup rollback failed: {}", failures.join("; "))
    }
}

pub(crate) fn is_internal_gateway_key(key: &GatewayKeyRecord) -> bool {
    key.system
        && (key.id == SYSTEM_GATEWAY_KEY_ID || key.id.starts_with(PROFILE_KEY_ROTATION_PREFIX))
}

pub(super) fn ensure_system_gateway_key(store: &Store, vault: &Vault) -> Result<(), String> {
    let existing = store
        .keys()?
        .into_iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID);
    let changed = existing
        .as_ref()
        .is_none_or(|key| !key.enabled || !key.system);
    let mut record = existing.unwrap_or_else(|| GatewayKeyRecord {
        id: SYSTEM_GATEWAY_KEY_ID.to_string(),
        label: "ChatGPT".to_string(),
        enabled: true,
        system: true,
        secret_ref: format!("key:{SYSTEM_GATEWAY_KEY_ID}"),
        created_at_ms: now_ms(),
        last_used_at_ms: None,
    });
    record.enabled = true;
    record.system = true;
    let created_secret = vault.load(&record.secret_ref)?.is_none();
    if created_secret {
        vault.save(&record.secret_ref, &generate_pool_key())?;
    }
    if changed {
        if let Err(error) = store.save_key(&record) {
            if created_secret {
                let _ = vault.delete(&record.secret_ref);
            }
            return Err(error);
        }
    }
    Ok(())
}

pub(crate) fn generate_pool_key() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("zrs_{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn identity_hint(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))[..12].to_string()
}

pub fn identity_fingerprint(server_id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("zenith-relay-server\0{server_id}").as_bytes(),
    ))
}

pub fn proxy_id(value: &str) -> String {
    zenith_relay_core::proxy_reference_id(value).expect("stored proxy URL was normalized")
}

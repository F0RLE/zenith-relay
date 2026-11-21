use crate::local_pool::{
    accounts::{NativeSecretBackend, credentials::CredentialStore},
    error::{ErrorCode, LocalPoolError},
    models::ProviderSourceRecord,
    state::DesktopState,
};
use sha2::{Digest, Sha256};
use zenith_relay_core::{
    merge_configuration_preset_settings, normalize_configuration_preset,
    protocol::{
        AccountPresetRule, CONFIGURATION_PRESET_FORMAT, CONFIGURATION_PRESET_SCHEMA_VERSION,
        ConfigurationPreset, ConfigurationPresetChange, ConfigurationPresetSettings,
        SourcePresetRule,
    },
    validate_resolved_configuration_preset_members,
};

use super::super::super::connections::validate_source_record;
use super::CommandResult;

pub(super) struct PreparedLocalPreset {
    pub(super) existing: ConfigurationPreset,
    pub(super) resolved: ConfigurationPreset,
    pub(super) target: ConfigurationPreset,
}

pub(super) fn local_preset_revision(preset: &ConfigurationPreset) -> CommandResult<String> {
    let bytes = serde_json::to_vec(preset).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("configuration preset could not be serialized: {error}"),
        )
    })?;
    Ok(format!("cfg_local_{}", hex::encode(Sha256::digest(bytes))))
}

pub(super) fn local_configuration_diff(
    before: &ConfigurationPreset,
    after: &ConfigurationPreset,
) -> CommandResult<Vec<ConfigurationPresetChange>> {
    let before = serde_json::to_value(before).map_err(LocalPoolError::invalid_state)?;
    let after = serde_json::to_value(after).map_err(LocalPoolError::invalid_state)?;
    let mut changes = Vec::new();
    diff_json("".into(), &before, &after, &mut changes);
    Ok(changes)
}

pub(super) fn prepare_local_configuration_preset(
    state: &DesktopState,
    preset: ConfigurationPreset,
) -> CommandResult<PreparedLocalPreset> {
    let existing = super::local_configuration_preset(state)?;
    let mut resolved = normalize_configuration_preset(preset)
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    resolve_local_preset_references(state, &mut resolved, &existing.settings)?;
    resolved.schema_version = CONFIGURATION_PRESET_SCHEMA_VERSION;
    resolved = normalize_configuration_preset(resolved)
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    if resolved.settings.quota.common_proxy_id != existing.settings.quota.common_proxy_id {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "configuration preset references a different common proxy",
        )
        .into());
    }
    let target = ConfigurationPreset {
        format: CONFIGURATION_PRESET_FORMAT.to_string(),
        schema_version: CONFIGURATION_PRESET_SCHEMA_VERSION,
        settings: merge_configuration_preset_settings(&existing.settings, &resolved.settings)
            .map_err(|message| LocalPoolError::new(ErrorCode::NotFound, message))?,
    };
    Ok(PreparedLocalPreset {
        existing,
        resolved,
        target,
    })
}

fn resolve_local_preset_references(
    state: &DesktopState,
    preset: &mut ConfigurationPreset,
    existing: &ConfigurationPresetSettings,
) -> CommandResult<()> {
    let settings = &mut preset.settings;
    let (sources, accounts) = {
        let store = state.store()?;
        (store.sources().to_vec(), store.accounts().to_vec())
    };
    let mut member_ids = std::collections::BTreeMap::new();
    for rule in &mut settings.sources {
        let source_record = resolve_local_source_reference(&sources, rule).ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::NotFound,
                format!(
                    "referenced source {} does not exist or is ambiguous",
                    rule.name
                ),
            )
        })?;
        let current = existing
            .sources
            .iter()
            .find(|r| r.id == source_record.id)
            .ok_or_else(|| {
                LocalPoolError::new(ErrorCode::Conflict, "source changed during preview")
            })?;
        validate_source_proxy_reference(rule, source_record, current, preset.schema_version)?;
        rule.proxy_id.clone_from(&current.proxy_id);
        validate_source_record(state, source_record)?;
        member_ids.insert(
            (zenith_relay_core::PoolMemberKind::Source, rule.id.clone()),
            source_record.id.clone(),
        );
        rule.apply_resolved_identity(
            &source_record.id,
            &source_record.name,
            &source_record.base_url,
            source_record.wire_api,
        );
    }
    settings
        .sources
        .sort_by(|left, right| left.id.cmp(&right.id));

    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    for rule in &mut settings.accounts {
        let account = resolve_local_account_reference(&accounts, rule).ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::NotFound,
                format!(
                    "referenced account {} does not exist or is ambiguous",
                    rule.identity_hint
                ),
            )
        })?;
        let credential = credentials.load(&account.account.id).map_err(|error| {
            LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
        })?;
        if rule.in_pool {
            crate::local_pool::accounts::records::validate_pool_account(
                credential.as_ref().ok_or_else(|| {
                    LocalPoolError::new(ErrorCode::NotFound, "pool account credentials are missing")
                })?,
                account.image_pool_enabled,
            )?;
        }
        let proxy_id = super::proxy_reference(credential.as_ref().and_then(|v| v.proxy_url()))?;
        if rule.proxy_id != proxy_id
            || rule.bypass_common_proxy != credential.is_some_and(|v| v.bypass_common_proxy())
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "configuration preset references a different account proxy",
            )
            .into());
        }
        member_ids.insert(
            (zenith_relay_core::PoolMemberKind::Account, rule.id.clone()),
            account.account.id.clone(),
        );
        rule.id = account.account.id.clone();
        rule.identity_hint = account
            .account
            .identity
            .identity_hash
            .chars()
            .take(12)
            .collect();
    }
    settings
        .accounts
        .sort_by(|left, right| left.id.cmp(&right.id));
    if let Some(policy) = &mut settings.routing.pool_routing {
        policy
            .remap_member_ids(&member_ids)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    }
    validate_resolved_configuration_preset_members(settings)
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    Ok(())
}

fn validate_source_proxy_reference(
    requested: &SourcePresetRule,
    source: &ProviderSourceRecord,
    current: &SourcePresetRule,
    schema_version: u16,
) -> CommandResult<()> {
    let same_legacy_binding = schema_version < 7
        && requested.proxy_id.is_some()
        && requested.id == source.id
        && requested.proxy_id == source.proxy_id;
    if requested.proxy_id == current.proxy_id || same_legacy_binding {
        Ok(())
    } else {
        Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "configuration preset references a different source proxy",
        )
        .into())
    }
}

fn resolve_local_source_reference<'a>(
    sources: &'a [ProviderSourceRecord],
    rule: &SourcePresetRule,
) -> Option<&'a ProviderSourceRecord> {
    sources
        .iter()
        .find(|source| {
            source.id == rule.id
                && source.wire_api == rule.wire_api
                && source.base_url.trim_end_matches('/') == rule.base_url
        })
        .or_else(|| unique_source_match(sources, rule, false))
        .or_else(|| unique_source_match(sources, rule, true))
}

fn unique_source_match<'a>(
    sources: &'a [ProviderSourceRecord],
    rule: &SourcePresetRule,
    match_name: bool,
) -> Option<&'a ProviderSourceRecord> {
    let mut matches = sources.iter().filter(|source| {
        source.wire_api == rule.wire_api
            && source.base_url.trim_end_matches('/') == rule.base_url
            && (!match_name || source.name == rule.name)
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn resolve_local_account_reference<'a>(
    accounts: &'a [crate::local_pool::models::LocalAccountRecord],
    rule: &AccountPresetRule,
) -> Option<&'a crate::local_pool::models::LocalAccountRecord> {
    accounts
        .iter()
        .find(|account| {
            account.account.id == rule.id && account_identity_hint(account) == rule.identity_hint
        })
        .or_else(|| {
            let mut matches = accounts
                .iter()
                .filter(|account| account_identity_hint(account) == rule.identity_hint);
            let first = matches.next()?;
            matches.next().is_none().then_some(first)
        })
}

fn account_identity_hint(account: &crate::local_pool::models::LocalAccountRecord) -> String {
    account
        .account
        .identity
        .identity_hash
        .chars()
        .take(12)
        .collect()
}

fn diff_json(
    path: String,
    before: &serde_json::Value,
    after: &serde_json::Value,
    changes: &mut Vec<ConfigurationPresetChange>,
) {
    match (before, after) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            let keys = left
                .keys()
                .chain(right.keys())
                .collect::<std::collections::BTreeSet<_>>();
            for key in keys {
                let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                match (left.get(key), right.get(key)) {
                    (Some(before), Some(after)) => diff_json(child, before, after, changes),
                    (before, after) => changes.push(ConfigurationPresetChange {
                        path: child,
                        before: before.cloned().unwrap_or(serde_json::Value::Null),
                        after: after.cloned().unwrap_or(serde_json::Value::Null),
                    }),
                }
            }
        }
        _ if before != after => changes.push(ConfigurationPresetChange {
            path: if path.is_empty() { "/".into() } else { path },
            before: before.clone(),
            after: after.clone(),
        }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_proxy_references_match_credentials_not_foreign_pool_ids() {
        let mut source = crate::local_pool::models::synthetic_responses_source();
        source.proxy_id = Some("proxy_local".into());
        let reference = |password| {
            zenith_relay_core::proxy_reference_id(&format!(
                "http://fixture:{password}@proxy.test:8100"
            ))
            .unwrap()
        };
        let current: SourcePresetRule = serde_json::from_value(serde_json::json!({
            "id": source.id, "name": source.name, "baseUrl": source.base_url, "wireApi": "responses",
            "enabled":true, "inPool":false, "allowedModels":[], "excludedModels":[], "priority":0,
            "weight":1, "proxyId":reference("one")
        })).unwrap();
        let mut requested = current.clone();
        requested.id = "source_elsewhere".into();
        assert!(validate_source_proxy_reference(&requested, &source, &current, 7).is_ok());
        for incompatible in [None, source.proxy_id.clone(), Some(reference("two"))] {
            requested.proxy_id = incompatible;
            assert!(validate_source_proxy_reference(&requested, &source, &current, 7).is_err());
        }
        requested.proxy_id.clone_from(&source.proxy_id);
        assert!(validate_source_proxy_reference(&requested, &source, &current, 6).is_err());
        requested.id.clone_from(&source.id);
        assert!(validate_source_proxy_reference(&requested, &source, &current, 6).is_ok());
    }
}

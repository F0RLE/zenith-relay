use super::PresetError;
use crate::state::{AccountCredential, AppState};
use std::collections::HashMap;
use zenith_relay_core::{
    protocol::ConfigurationPresetSettings, validate_resolved_configuration_preset_members,
    ProxyConfig,
};
pub(super) fn resolve_references(
    state: &AppState,
    settings: &mut ConfigurationPresetSettings,
) -> Result<(), PresetError> {
    let sources = state.store.sources().map_err(PresetError::Store)?;
    let mut member_ids = std::collections::BTreeMap::new();
    for rule in &mut settings.sources {
        let record = sources
            .iter()
            .find(|record| {
                record.id == rule.id
                    && record.wire_api == rule.wire_api
                    && record.base_url.trim_end_matches('/') == rule.base_url
            })
            .or_else(|| {
                let mut matches = sources.iter().filter(|record| {
                    record.wire_api == rule.wire_api
                        && record.base_url.trim_end_matches('/') == rule.base_url
                });
                let first = matches.next();
                if matches.next().is_none() {
                    return first;
                }
                let mut named = sources.iter().filter(|record| {
                    record.name == rule.name
                        && record.wire_api == rule.wire_api
                        && record.base_url.trim_end_matches('/') == rule.base_url
                });
                let first = named.next();
                (named.next().is_none()).then_some(first).flatten()
            })
            .ok_or_else(|| {
                PresetError::Missing(format!(
                    "referenced source {} does not exist or is ambiguous",
                    rule.name
                ))
            })?;
        member_ids.insert(
            (zenith_relay_core::PoolMemberKind::Source, rule.id.clone()),
            record.id.clone(),
        );
        rule.id = record.id.clone();
        rule.name = record.name.clone();
        rule.base_url = record.base_url.trim_end_matches('/').to_string();
        rule.wire_api = record.wire_api;
    }
    settings
        .sources
        .sort_by(|left, right| left.id.cmp(&right.id));

    let accounts = state.store.accounts().map_err(PresetError::Store)?;
    for rule in &mut settings.accounts {
        let record = accounts
            .iter()
            .find(|record| record.id == rule.id && record.identity_hint == rule.identity_hint)
            .or_else(|| {
                let mut matches = accounts
                    .iter()
                    .filter(|record| record.identity_hint == rule.identity_hint);
                let first = matches.next();
                (matches.next().is_none()).then_some(first).flatten()
            })
            .ok_or_else(|| {
                PresetError::Missing(format!(
                    "referenced account {} does not exist or is ambiguous",
                    rule.identity_hint
                ))
            })?;
        member_ids.insert(
            (zenith_relay_core::PoolMemberKind::Account, rule.id.clone()),
            record.id.clone(),
        );
        rule.id = record.id.clone();
        rule.identity_hint = record.identity_hint.clone();
    }
    settings
        .accounts
        .sort_by(|left, right| left.id.cmp(&right.id));
    if let Some(policy) = &mut settings.routing.pool_routing {
        policy
            .remap_member_ids(&member_ids)
            .map_err(|message| PresetError::Invalid(message.into()))?;
    }
    validate_resolved_configuration_preset_members(settings).map_err(PresetError::Invalid)?;
    Ok(())
}

pub(super) fn validate_references(
    state: &AppState,
    settings: &ConfigurationPresetSettings,
) -> Result<(), PresetError> {
    let sources = state
        .store
        .sources()
        .map_err(PresetError::Store)?
        .into_iter()
        .map(|record| (record.id.clone(), record))
        .collect::<HashMap<_, _>>();
    for rule in &settings.sources {
        let record = sources.get(&rule.id).ok_or_else(|| {
            PresetError::Missing(format!("referenced source {} does not exist", rule.id))
        })?;
        if state
            .vault
            .load(&record.secret_ref)
            .map_err(PresetError::Store)?
            .is_none()
        {
            return Err(PresetError::Missing(format!(
                "referenced source {} has no stored credential",
                rule.id
            )));
        }
    }
    let accounts = state
        .store
        .accounts()
        .map_err(PresetError::Store)?
        .into_iter()
        .map(|record| (record.id.clone(), record))
        .collect::<HashMap<_, _>>();
    for rule in &settings.accounts {
        let record = accounts.get(&rule.id).ok_or_else(|| {
            PresetError::Missing(format!("referenced account {} does not exist", rule.id))
        })?;
        let credential = state
            .vault
            .load(&record.secret_ref)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!(
                    "referenced account {} has no stored credential",
                    rule.id
                ))
            })?;
        serde_json::from_str::<AccountCredential>(&credential).map_err(|_| {
            PresetError::Missing(format!(
                "referenced account {} has an invalid credential",
                rule.id
            ))
        })?;
    }
    for proxy_id in settings
        .accounts
        .iter()
        .filter_map(|rule| rule.proxy_id.as_deref())
        .chain(settings.quota.common_proxy_id.as_deref())
    {
        let record = state
            .store
            .proxy(proxy_id)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!("referenced proxy {proxy_id} does not exist"))
            })?;
        let secret = state
            .vault
            .load(&record.secret_ref)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!("referenced proxy {proxy_id} has no stored secret"))
            })?;
        ProxyConfig::parse(&secret)
            .map_err(|_| PresetError::Missing(format!("referenced proxy {proxy_id} is invalid")))?;
    }
    Ok(())
}

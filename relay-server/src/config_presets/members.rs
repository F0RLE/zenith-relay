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
        let source_record = sources
            .iter()
            .find(|candidate_source| {
                candidate_source.id == rule.id
                    && candidate_source.wire_api == rule.wire_api
                    && candidate_source.base_url.trim_end_matches('/') == rule.base_url
            })
            .or_else(|| {
                let mut matches = sources.iter().filter(|candidate_source| {
                    candidate_source.wire_api == rule.wire_api
                        && candidate_source.base_url.trim_end_matches('/') == rule.base_url
                });
                let first = matches.next();
                if matches.next().is_none() {
                    return first;
                }
                let mut named = sources.iter().filter(|candidate_source| {
                    candidate_source.name == rule.name
                        && candidate_source.wire_api == rule.wire_api
                        && candidate_source.base_url.trim_end_matches('/') == rule.base_url
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

    let accounts = state.store.accounts().map_err(PresetError::Store)?;
    for rule in &mut settings.accounts {
        let account_record = accounts
            .iter()
            .find(|candidate_account| {
                candidate_account.id == rule.id
                    && candidate_account.identity_hint == rule.identity_hint
            })
            .or_else(|| {
                let mut matches = accounts.iter().filter(|candidate_account| {
                    candidate_account.identity_hint == rule.identity_hint
                });
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
            account_record.id.clone(),
        );
        rule.id = account_record.id.clone();
        rule.identity_hint = account_record.identity_hint.clone();
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
        .map(|source_record| (source_record.id.clone(), source_record))
        .collect::<HashMap<_, _>>();
    for rule in &settings.sources {
        let source_record = sources.get(&rule.id).ok_or_else(|| {
            PresetError::Missing(format!("referenced source {} does not exist", rule.id))
        })?;
        if state
            .vault
            .load(&source_record.secret_ref)
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
        .map(|account_record| (account_record.id.clone(), account_record))
        .collect::<HashMap<_, _>>();
    for rule in &settings.accounts {
        let account_record = accounts.get(&rule.id).ok_or_else(|| {
            PresetError::Missing(format!("referenced account {} does not exist", rule.id))
        })?;
        let credential = state
            .vault
            .load(&account_record.secret_ref)
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
        let proxy_record = state
            .store
            .proxy(proxy_id)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!("referenced proxy {proxy_id} does not exist"))
            })?;
        let secret = state
            .vault
            .load(&proxy_record.secret_ref)
            .map_err(PresetError::Store)?
            .ok_or_else(|| {
                PresetError::Missing(format!("referenced proxy {proxy_id} has no stored secret"))
            })?;
        ProxyConfig::parse(&secret)
            .map_err(|_| PresetError::Missing(format!("referenced proxy {proxy_id} is invalid")))?;
    }
    Ok(())
}

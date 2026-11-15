use crate::local_pool::models::{GatewaySettings, LocalAccountRecord};
use crate::local_pool::state::AccountCredentialFacts;
use std::collections::HashMap;
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    automations::{WakeTask, WakeTrigger},
    providers::chatgpt::OAuthClientKind,
};

fn weekly_reset_registration_eligible(record: &LocalAccountRecord) -> bool {
    let account = &record.account;
    record.recovery.is_none()
        && account.enabled
        && !account.draining
        && account.auth_state == AccountAuthState::Active
        && (account.health == AccountHealthState::Healthy
            || (account.health == AccountHealthState::Degraded
                && account.last_error_code.as_deref() == Some("upstream_route_degraded")))
}

/// A generation-route failure does not revoke authenticated quota operations.
/// Reset admission still checks current ledger evidence and the matching rule.
pub(crate) fn weekly_reset_pool_member_eligible(record: &LocalAccountRecord) -> bool {
    record.account.in_pool && weekly_reset_registration_eligible(record)
}

/// An explicitly linked reader may serve the selected pool connection without
/// becoming a dispatch member or acquiring its credentials.
pub(crate) fn selected_pool_account<'a>(
    settings: &GatewaySettings,
    accounts: &'a [LocalAccountRecord],
    reader: &LocalAccountRecord,
) -> Option<&'a LocalAccountRecord> {
    if !weekly_reset_registration_eligible(reader) {
        return None;
    }
    let selected_id = settings
        .account_routes
        .iter()
        .find(|group| group.account_ids.contains(&reader.account.id))
        .map_or(&reader.account.id, |group| &group.selected_account_id);
    accounts.iter().find(|record| {
        record.account.id == *selected_id && weekly_reset_pool_member_eligible(record)
    })
}

pub(crate) fn weekly_reset_rule_matches(
    task: &WakeTask,
    reader: &LocalAccountRecord,
    selected: &LocalAccountRecord,
) -> bool {
    task.enabled
        && task.trigger == WakeTrigger::Weekly
        && (task.account_selector.matches(&reader.account)
            || task.account_selector.matches(&selected.account))
}

/// Keep one automatic redeemer per linked ledger even when several native
/// connections observe the same exhausted week. Prefer the selected reader.
pub(crate) fn weekly_reset_member<'a>(
    settings: &GatewaySettings,
    accounts: &'a [LocalAccountRecord],
    reader: &LocalAccountRecord,
    credentials: &HashMap<String, Option<AccountCredentialFacts>>,
) -> Option<&'a LocalAccountRecord> {
    let selected = selected_pool_account(settings, accounts, reader)?;
    let Some(group) = settings
        .account_routes
        .iter()
        .find(|group| group.account_ids.contains(&reader.account.id))
    else {
        return Some(selected);
    };
    let owner = accounts
        .iter()
        .filter(|candidate| {
            group.account_ids.contains(&candidate.account.id)
                && credentials
                    .get(&candidate.account.id)
                    .and_then(Option::as_ref)
                    .is_some_and(|facts| {
                        matches!(
                            facts.oauth_client_kind,
                            OAuthClientKind::Codex | OAuthClientKind::ExcelBps
                        ) && (facts.has_oauth || facts.agent_identity)
                    })
                && selected_pool_account(settings, accounts, candidate).is_some()
        })
        .min_by_key(|candidate| {
            (
                candidate.account.id != selected.account.id,
                &candidate.account.id,
            )
        })?;
    (owner.account.id == reader.account.id).then_some(selected)
}

/// Attempt history belongs to the linked ledger, even after switching its
/// selected connection or disabling the previous reader.
pub(crate) fn linked_reset_attempts<'a>(
    settings: &'a GatewaySettings,
    records: &'a crate::local_pool::models::AutomationRecords,
    account_id: &'a str,
) -> impl Iterator<Item = &'a str> {
    let group = settings.account_route_group(account_id);
    records
        .weekly_reset_fingerprints
        .iter()
        .filter_map(move |(id, marker)| {
            (id == account_id || group.is_some_and(|group| group.account_ids.contains(id)))
                .then_some(marker.as_str())
        })
}

#[cfg(test)]
pub(in crate::local_pool) mod tests {
    use super::*;
    use crate::local_pool::{models::AccountRouteGroup, refresh::tests::account};

    pub(in crate::local_pool) fn native_facts(
        accounts: &[LocalAccountRecord],
    ) -> HashMap<String, Option<AccountCredentialFacts>> {
        accounts
            .iter()
            .map(|account| {
                (
                    account.account.id.clone(),
                    Some(AccountCredentialFacts {
                        credit_balance_key: None,
                        oauth_client_kind: OAuthClientKind::Codex,
                        has_oauth: true,
                        has_refresh_token: true,
                        plan_use_enabled: true,
                        expires_at_ms: None,
                        agent_identity: false,
                        has_provider_account_id: true,
                        has_account_proxy: false,
                        account_proxy_valid: false,
                        bypass_common_proxy: false,
                    }),
                )
            })
            .collect()
    }

    #[test]
    fn route_degradation_preserves_reset_eligibility_without_enabling_generation_wakes() {
        let mut member = account();
        member.account.in_pool = true;
        member.account.health = AccountHealthState::Degraded;
        member.account.last_error_code = Some("upstream_route_degraded".into());
        let records = vec![member];
        let credentials = native_facts(&records);
        let settings = GatewaySettings::default();
        assert!(weekly_reset_member(&settings, &records, &records[0], &credentials).is_some());
        assert!(!records[0].account.is_wake_eligible());
        assert_eq!(records[0].account.health, AccountHealthState::Degraded);

        for gate in 0..9 {
            let mut blocked = records.clone();
            let account = &mut blocked[0].account;
            match gate {
                0 => account.enabled = false,
                1 => account.draining = true,
                2 => account.in_pool = false,
                3 => {
                    account.auth_state = AccountAuthState::RequiresReauth(
                        zenith_relay_core::accounts::ReauthReason::InvalidGrant,
                    )
                }
                4 => account.health = AccountHealthState::Unknown,
                5 => account.health = AccountHealthState::Unhealthy,
                6 => account.health = AccountHealthState::Blocked,
                7 => account.last_error_code = None,
                _ => account.last_error_code = Some("quota_refresh_failed".into()),
            }
            assert!(
                weekly_reset_member(&settings, &blocked, &blocked[0], &credentials).is_none(),
                "gate {gate}"
            );
        }
    }

    #[test]
    fn linked_reset_elects_one_native_reader_without_requiring_prior_quota() {
        let mut selected = account();
        selected.account.id = "selected".into();
        selected.account.in_pool = true;
        let mut reader = selected.clone();
        reader.account.id = "native-a".into();
        reader.account.in_pool = false;
        let mut alternate = reader.clone();
        alternate.account.id = "native-b".into();
        let mut records = vec![selected, reader, alternate];
        let mut credentials = native_facts(&records);
        credentials
            .get_mut("selected")
            .unwrap()
            .as_mut()
            .unwrap()
            .oauth_client_kind = OAuthClientKind::Siwc;
        let mut settings = GatewaySettings {
            account_routes: vec![AccountRouteGroup {
                account_ids: records
                    .iter()
                    .map(|record| record.account.id.clone())
                    .collect(),
                selected_account_id: "selected".into(),
                notes_account_id: None,
            }],
            ..Default::default()
        };
        let owners = |settings: &GatewaySettings, records: &[LocalAccountRecord]| {
            records
                .iter()
                .filter(|record| {
                    weekly_reset_member(settings, records, record, &credentials).is_some()
                })
                .map(|record| record.account.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(owners(&settings, &records), ["native-a"]);
        records.swap(1, 2);
        assert_eq!(owners(&settings, &records), ["native-a"]);
        records[2].account.enabled = false;
        assert_eq!(owners(&settings, &records), ["native-b"]);
        records[0].account.in_pool = false;
        assert!(owners(&settings, &records).is_empty());
        records[1].account.in_pool = true;
        settings.account_routes[0].selected_account_id = "native-b".into();
        assert_eq!(owners(&settings, &records), ["native-b"]);
        records[1].account.auth_state = AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::InvalidGrant,
        );
        assert!(owners(&settings, &records).is_empty());
    }

    #[test]
    fn linked_reader_requires_an_explicit_usable_selected_pool_member() {
        let mut reader = account();
        reader.account.id = "reader".into();
        reader.account.in_pool = false;
        let mut selected = reader.clone();
        selected.account.id = "selected".into();
        selected.account.in_pool = true;
        let settings = GatewaySettings {
            account_routes: vec![AccountRouteGroup {
                account_ids: vec![reader.account.id.clone(), selected.account.id.clone()],
                selected_account_id: selected.account.id.clone(),
                notes_account_id: None,
            }],
            ..Default::default()
        };
        let mut records = vec![reader.clone(), selected];
        assert_eq!(
            selected_pool_account(&settings, &records, &reader)
                .map(|record| record.account.id.as_str()),
            Some("selected")
        );
        assert!(selected_pool_account(&GatewaySettings::default(), &records, &reader).is_none());
        for record in &mut records {
            record.account.health = AccountHealthState::Degraded;
            record.account.last_error_code = Some("upstream_route_degraded".into());
        }
        assert_eq!(
            selected_pool_account(&settings, &records, &records[0])
                .map(|record| record.account.id.as_str()),
            Some("selected")
        );
        for gate in 0..5 {
            let mut blocked = records.clone();
            match gate {
                0 => blocked[1].account.in_pool = false,
                1 => blocked[1].account.enabled = false,
                2 => blocked[1].account.draining = true,
                3 => blocked[0].account.enabled = false,
                _ => blocked[0].account.draining = true,
            }
            assert!(selected_pool_account(&settings, &blocked, &blocked[0]).is_none());
        }
        assert!(!records[0].account.in_pool);
    }
}

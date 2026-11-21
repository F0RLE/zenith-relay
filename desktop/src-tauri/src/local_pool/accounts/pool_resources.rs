use crate::local_pool::models::{GatewaySettings, LocalAccountRecord};
use crate::local_pool::state::AccountCredentialFacts;
use std::collections::HashMap;
#[cfg(test)]
use zenith_relay_core::accounts::{AccountAuthState, AccountHealthState};
use zenith_relay_core::automations::WakeTask;
#[cfg(test)]
use zenith_relay_core::providers::chatgpt::OAuthClientKind;
use zenith_relay_core::providers::chatgpt::connector::reset_credits::supports_reset_credits;

/// An explicitly linked reader may serve the selected pool connection without
/// becoming a dispatch member or acquiring its credentials.
#[cfg(test)]
pub(crate) fn selected_pool_account<'a>(
    settings: &GatewaySettings,
    accounts: &'a [LocalAccountRecord],
    reader: &LocalAccountRecord,
) -> Option<&'a LocalAccountRecord> {
    zenith_relay_core::automations::reset_resources::selected_pool_account(
        &settings.account_routes,
        accounts,
        reader,
    )
}

pub(crate) fn weekly_reset_rule_matches(
    task: &WakeTask,
    reader: &LocalAccountRecord,
    selected: &LocalAccountRecord,
) -> bool {
    zenith_relay_core::automations::reset_resources::rule_matches(task, reader, selected)
}

/// Keep one automatic redeemer per linked ledger even when several native
/// connections observe the same exhausted week. Prefer the selected reader.
pub(crate) fn weekly_reset_member<'a>(
    settings: &GatewaySettings,
    accounts: &'a [LocalAccountRecord],
    reader: &LocalAccountRecord,
    credentials: &HashMap<String, Option<AccountCredentialFacts>>,
) -> Option<&'a LocalAccountRecord> {
    let standalone = settings.account_route_group(&reader.account.id).is_none();
    zenith_relay_core::automations::reset_resources::reset_member(
        &settings.account_routes,
        accounts,
        reader,
        |id| {
            let facts = credentials.get(id).and_then(Option::as_ref);
            // A standalone ledger has no competing reader to elect. Its saved
            // connector can establish capability without opening credentials;
            // dispatch still prepares and fences current authorization.
            if standalone {
                return facts
                    .map(|facts| facts.oauth_client_kind)
                    .or(reader.oauth_client_kind)
                    .is_some_and(supports_reset_credits);
            }
            facts.is_some_and(|facts| {
                supports_reset_credits(facts.oauth_client_kind)
                    && (facts.has_oauth || facts.agent_identity)
            })
        },
    )
}

#[cfg(test)]
pub(in crate::local_pool) mod tests {
    use super::*;
    use crate::local_pool::{models::AccountRouteGroup, refresh::tests::account};

    #[test]
    fn standalone_ledger_keeps_capability_separate_from_credential_availability() {
        let mut reader = account();
        reader.account.in_pool = true;
        let settings = GatewaySettings::default();
        let facts = HashMap::new();
        assert!(weekly_reset_member(&settings, &[reader.clone()], &reader, &facts).is_some());
        for kind in [Some(OAuthClientKind::Siwc), None] {
            reader.oauth_client_kind = kind;
            assert!(weekly_reset_member(&settings, &[reader.clone()], &reader, &facts).is_none());
        }
    }

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

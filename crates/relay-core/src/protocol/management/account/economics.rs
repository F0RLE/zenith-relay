use super::AccountSummary;
use crate::{ApiEquivalentSummary, providers::chatgpt::OAuthClientKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMethodUsage {
    pub client_kind: OAuthClientKind,
    pub api_equivalent: ApiEquivalentSummary,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountEconomics {
    pub group_id: String,
    pub api_equivalent: ApiEquivalentSummary,
    pub methods: Vec<AccountMethodUsage>,
    pub purchase_cost_micro_usd: Option<u64>,
}

/// Reuse persisted, validated route associations. Similar labels, emails and
/// shared quota observations do not create an account association here.
pub fn project_account_economics(accounts: &mut [AccountSummary]) {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, account) in accounts.iter().enumerate() {
        let key = account.route_group_id.as_ref().map_or_else(
            || format!("account:{}", account.id),
            |id| format!("route:{id}"),
        );
        groups.entry(key).or_default().push(index);
    }
    for (group_id, indexes) in groups {
        let mut economics = AccountEconomics {
            group_id,
            api_equivalent: ApiEquivalentSummary::default(),
            methods: Vec::new(),
            purchase_cost_micro_usd: None,
        };
        let mut seen = BTreeSet::new();
        for &index in &indexes {
            let account = &accounts[index];
            if !seen.insert(&account.id) {
                continue;
            }
            economics.api_equivalent.merge(account.api_equivalent);
            if !account.route_inactive {
                economics.purchase_cost_micro_usd = account.purchase_cost_micro_usd;
            }
        }
        for kind in [
            OAuthClientKind::Codex,
            OAuthClientKind::ExcelBps,
            OAuthClientKind::Siwc,
        ] {
            let mut usage = None::<ApiEquivalentSummary>;
            let mut seen = BTreeSet::new();
            for &index in &indexes {
                let account = &accounts[index];
                if account.oauth_client_kind == Some(kind) && seen.insert(&account.id) {
                    usage.get_or_insert_default().merge(account.api_equivalent);
                }
            }
            if let Some(api_equivalent) = usage {
                economics.methods.push(AccountMethodUsage {
                    client_kind: kind,
                    api_equivalent,
                });
            }
        }
        for index in indexes {
            accounts[index].economics = Some(economics.clone());
        }
    }
}

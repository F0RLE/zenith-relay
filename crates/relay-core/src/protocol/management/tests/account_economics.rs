use super::*;
use crate::providers::chatgpt::OAuthClientKind;

fn connection(id: &str, kind: OAuthClientKind, micro_usd: u64) -> AccountSummary {
    let mut account = account_summary(false, &[]);
    account.id = id.into();
    account.route_group_id = Some("linked".into());
    account.oauth_client_kind = Some(kind);
    account.route_inactive = kind != OAuthClientKind::Siwc;
    account.api_equivalent = ApiEquivalentSummary {
        micro_usd,
        priced_tokens: 10,
        unpriced_tokens: 0,
    };
    account.purchase_cost_micro_usd = Some(100);
    account
}

#[test]
fn account_economics_combines_methods_once_without_changing_route_usage() {
    let mut accounts = vec![
        connection("codex", OAuthClientKind::Codex, 20),
        connection("bps", OAuthClientKind::ExcelBps, 30),
        connection("siwc", OAuthClientKind::Siwc, 40),
    ];
    project_account_economics(&mut accounts);
    let shared = accounts[0].economics.as_ref().unwrap();
    assert_eq!(shared.api_equivalent.micro_usd, 90);
    assert_eq!(shared.api_equivalent.priced_tokens, 30);
    assert_eq!(shared.purchase_cost_micro_usd, Some(100));
    assert_eq!(shared.methods.len(), 3);
    assert_eq!(
        shared
            .methods
            .iter()
            .map(|method| method.api_equivalent.micro_usd)
            .sum::<u64>(),
        90
    );
    assert!(
        accounts
            .iter()
            .all(|account| account.economics.as_ref() == Some(shared))
    );
    assert_eq!(accounts[0].api_equivalent.micro_usd, 20);
    let before = accounts.clone();
    project_account_economics(&mut accounts);
    assert_eq!(accounts, before);
}

#[test]
fn unknown_client_history_is_retained_without_attributing_it_to_codex() {
    let mut account = connection("unknown", OAuthClientKind::Siwc, 150);
    account.oauth_client_kind = None;
    let mut accounts = vec![account];
    project_account_economics(&mut accounts);
    let economics = accounts[0].economics.as_ref().unwrap();
    assert_eq!(economics.api_equivalent.micro_usd, 150);
    assert!(economics.methods.is_empty());
}

#[test]
fn account_economics_retains_inactive_reauthentication_and_unpriced_history() {
    let mut accounts = vec![
        connection("old", OAuthClientKind::ExcelBps, 30),
        connection("active", OAuthClientKind::Siwc, 40),
    ];
    accounts[0].secret_available = false;
    accounts[0].recovery_required = true;
    accounts[0].enabled = false;
    accounts[0].api_equivalent.unpriced_tokens = 12;
    accounts[1].purchase_cost_micro_usd = Some(150);
    project_account_economics(&mut accounts);
    let shared = accounts[1].economics.as_ref().unwrap();
    assert_eq!(shared.api_equivalent.micro_usd, 70);
    assert_eq!(shared.api_equivalent.unpriced_tokens, 12);
    assert_eq!(shared.purchase_cost_micro_usd, Some(150));
    assert_eq!(shared.methods[0].api_equivalent.unpriced_tokens, 12);
}

#[test]
fn account_economics_does_not_infer_links_from_labels_or_credit_ledgers() {
    let mut accounts = vec![
        connection("a", OAuthClientKind::Codex, 20),
        connection("b", OAuthClientKind::Siwc, 40),
    ];
    for account in &mut accounts {
        account.route_group_id = None;
        account.credit_balance_key = Some("shared-ledger".into());
    }
    project_account_economics(&mut accounts);
    let first = accounts[0].economics.as_ref().unwrap();
    let second = accounts[1].economics.as_ref().unwrap();
    assert_eq!(first.api_equivalent.micro_usd, 20);
    assert_eq!(second.api_equivalent.micro_usd, 40);
    assert_ne!(first.group_id, second.group_id);
}

#[test]
fn account_economics_groups_repeated_clients_and_preserves_unknown_costs() {
    let mut accounts = vec![
        connection("old", OAuthClientKind::Siwc, 20),
        connection("new", OAuthClientKind::Siwc, 40),
    ];
    accounts[0].route_inactive = true;
    accounts[0].purchase_cost_micro_usd = None;
    accounts[1].purchase_cost_micro_usd = None;
    project_account_economics(&mut accounts);
    let shared = accounts[1].economics.as_ref().unwrap();
    assert_eq!(shared.methods.len(), 1);
    assert_eq!(shared.methods[0].api_equivalent.micro_usd, 60);
    assert_eq!(shared.purchase_cost_micro_usd, None);
}

#[test]
fn account_economics_does_not_restore_a_cleared_purchase_cost_from_inactive_routes() {
    let mut accounts = vec![
        connection("old", OAuthClientKind::ExcelBps, 20),
        connection("new", OAuthClientKind::Siwc, 40),
    ];
    accounts[1].purchase_cost_micro_usd = None;
    project_account_economics(&mut accounts);
    for account in accounts {
        assert_eq!(account.economics.unwrap().purchase_cost_micro_usd, None);
    }
}

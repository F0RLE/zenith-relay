//! Account mutations shared by hosts; persistence and dispatch fencing stay with the host.
use super::{AccountRecord, AccountRouteGroup};
use crate::{PoolRoutingPolicy, providers::chatgpt::OAuthClientKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountRouteSettings {
    pub priority: i32,
    pub weight: u32,
    pub purchase_cost_micro_usd: Option<u64>,
}

/// A host record exposes only the state needed by shared account policies.
pub trait ManagedAccount: Clone {
    fn account(&self) -> &AccountRecord;
    fn account_mut(&mut self) -> &mut AccountRecord;
    fn requires_recovery(&self) -> bool;
    fn route_settings(&self) -> AccountRouteSettings;
    fn set_route_settings(&mut self, settings: AccountRouteSettings);
    fn images_enabled(&self) -> bool;
    fn set_images_enabled(&mut self, enabled: bool);
}

pub fn copy_route_settings<A: ManagedAccount>(destination: &mut A, source: &A) {
    let source_record = source.account();
    let target = destination.account_mut();
    target.label = source_record.label.clone();
    target.tags = source_record.tags.clone();
    target.enabled = source_record.enabled;
    target.draining = source_record.draining;
    target.in_pool = source_record.in_pool;
    destination.set_route_settings(source.route_settings());
}

/// Plan on copies so validation failure never leaves a partially changed inventory.
/// The host must keep its mutation lock and dispatch fences through commit/rollback.
pub fn select_route_records<A: ManagedAccount>(
    accounts: &mut [A],
    groups: &mut Vec<AccountRouteGroup>,
    policy: &mut Option<PoolRoutingPolicy>,
    current: &str,
    next_kind: OAuthClientKind,
    group: AccountRouteGroup,
) -> Result<Vec<String>, &'static str> {
    super::validate_account_route_groups(std::slice::from_ref(&group))?;
    if !group.account_ids.iter().any(|id| id == current)
        || group
            .account_ids
            .iter()
            .any(|id| !accounts.iter().any(|account| account.account().id == *id))
    {
        return Err("Account route was not found");
    }
    if groups.iter().any(|group| {
        group.account_ids.iter().any(|id| id == current) && group.selected_account_id != current
    }) {
        return Err("The selected account route changed; refresh and try again");
    }
    let mut proposed = accounts.to_vec();
    let source = proposed
        .iter()
        .find(|a| a.account().id == current)
        .cloned()
        .ok_or("Account route was not found")?;
    let destination = proposed
        .iter_mut()
        .find(|a| a.account().id == group.selected_account_id)
        .ok_or("Account route was not found")?;
    if source.requires_recovery() || destination.requires_recovery() {
        return Err("Recover the account before changing its route");
    }
    copy_route_settings(destination, &source);
    destination.set_images_enabled(false);
    destination.account_mut().in_pool &= next_kind != OAuthClientKind::Codex;
    for account in &mut proposed {
        if group.account_ids.contains(&account.account().id)
            && account.account().id != group.selected_account_id
        {
            account.account_mut().in_pool = false;
            account.set_images_enabled(false);
        }
    }
    if let Some(policy) = policy {
        group.retarget_pool_routing(policy, current, next_kind);
    }
    groups.retain(|existing| {
        !existing
            .account_ids
            .iter()
            .any(|id| group.account_ids.contains(id))
    });
    let ids = group.account_ids.clone();
    groups.push(group);
    accounts.clone_from_slice(&proposed);
    Ok(ids)
}

pub fn restore_route_records<A: ManagedAccount>(
    current: &mut [A],
    previous: &[A],
) -> Result<(), &'static str> {
    let mut restored = current.to_vec();
    for previous in previous {
        let destination = restored
            .iter_mut()
            .find(|record| {
                record.account().id == previous.account().id
                    && record.account().identity.identity_hash
                        == previous.account().identity.identity_hash
            })
            .ok_or("Account changed during route rollback")?;
        copy_route_settings(destination, previous);
        destination.set_images_enabled(previous.images_enabled());
    }
    current.clone_from_slice(&restored);
    Ok(())
}

pub struct PoolMembershipEvidence<'a> {
    pub requested_accounts: &'a std::collections::BTreeSet<String>,
    pub requested_sources: &'a std::collections::BTreeSet<String>,
    pub known_source_ids: &'a [&'a str],
    pub verified_source_ids: &'a [&'a str],
    pub routes: &'a [AccountRouteGroup],
    pub in_pool: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipError {
    Empty,
    Missing,
    RecoveryRequired,
    UnselectedRoute,
    UnverifiedSource,
}

pub fn validate_pool_membership<A: ManagedAccount>(
    accounts: &[A],
    evidence: &PoolMembershipEvidence<'_>,
) -> Result<(), MembershipError> {
    let PoolMembershipEvidence {
        requested_accounts,
        requested_sources,
        known_source_ids,
        verified_source_ids,
        routes,
        in_pool,
    } = evidence;
    if requested_accounts.is_empty() && requested_sources.is_empty() {
        return Err(MembershipError::Empty);
    }
    if requested_accounts
        .iter()
        .any(|id| !accounts.iter().any(|account| account.account().id == *id))
        || requested_sources
            .iter()
            .any(|id| !known_source_ids.contains(&id.as_str()))
    {
        return Err(MembershipError::Missing);
    }
    if !in_pool {
        return Ok(());
    }
    if accounts
        .iter()
        .any(|a| requested_accounts.contains(&a.account().id) && a.requires_recovery())
    {
        return Err(MembershipError::RecoveryRequired);
    }
    if requested_accounts.iter().any(|id| {
        routes
            .iter()
            .any(|group| group.account_ids.contains(id) && group.selected_account_id != *id)
    }) {
        return Err(MembershipError::UnselectedRoute);
    }
    if requested_sources
        .iter()
        .any(|id| !verified_source_ids.contains(&id.as_str()))
    {
        return Err(MembershipError::UnverifiedSource);
    }
    Ok(())
}

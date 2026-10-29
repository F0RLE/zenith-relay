use crate::providers::chatgpt::OAuthClientKind;
use crate::{PoolMemberKind, PoolRoutingPolicy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRouteGroup {
    pub account_ids: Vec<String>,
    pub selected_account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_account_id: Option<String>,
}

impl AccountRouteGroup {
    pub fn retarget_pool_routing(
        &self,
        policy: &mut PoolRoutingPolicy,
        previous: &str,
        next_kind: OAuthClientKind,
    ) {
        policy.members.retain(|member| {
            member.kind != PoolMemberKind::Account
                || (member.id != self.selected_account_id
                    && (next_kind != OAuthClientKind::Codex
                        || !self.account_ids.contains(&member.id)))
        });
        for member in &mut policy.members {
            if member.kind == PoolMemberKind::Account && member.id == previous {
                member.id.clone_from(&self.selected_account_id);
            }
        }
    }
}

/// Facts from protected grants, not editable login notes or entitlement evidence.
pub struct AccountRouteIdentity {
    pub client_kind: OAuthClientKind,
    pub provider_account_id: Option<String>,
    pub provider_user_id: Option<String>,
    /// The caller must pin this email to the saved sign-in identity.
    pub original_login_email: Option<String>,
}

impl AccountRouteIdentity {
    pub fn same_owner(&self, other: &Self) -> bool {
        if self.client_kind == other.client_kind {
            return false;
        }
        if self.client_kind != OAuthClientKind::Siwc && other.client_kind != OAuthClientKind::Siwc {
            return self.provider_account_id.is_some()
                && self.provider_account_id == other.provider_account_id
                && self.provider_user_id.is_some()
                && self.provider_user_id == other.provider_user_id;
        }
        self.original_login_email
            .as_deref()
            .zip(other.original_login_email.as_deref())
            .is_some_and(|(left, right)| {
                !left.trim().is_empty() && left.trim().eq_ignore_ascii_case(right.trim())
            })
    }
}

pub fn validate_account_route_connection(
    connections: impl IntoIterator<Item = AccountRouteIdentity>,
    candidate: &AccountRouteIdentity,
) -> Result<(), &'static str> {
    for existing in connections {
        if existing.client_kind == candidate.client_kind {
            return Err("This account already has a connection of this type");
        }
        if !existing.same_owner(candidate) {
            return Err(
                "Authorize a different route for the same account and workspace before associating it",
            );
        }
    }
    Ok(())
}

pub fn validate_account_route_groups(groups: &[AccountRouteGroup]) -> Result<(), &'static str> {
    let mut route_accounts = BTreeSet::new();
    for group in groups {
        if !(2..=3).contains(&group.account_ids.len())
            || !group.account_ids.contains(&group.selected_account_id)
            || group
                .notes_account_id
                .as_ref()
                .is_some_and(|id| !group.account_ids.contains(id))
            || group
                .account_ids
                .iter()
                .any(|id| id.is_empty() || !route_accounts.insert(id))
        {
            return Err(
                "account route groups must contain distinct connections and one selected route",
            );
        }
    }
    Ok(())
}

/// Plan a selection without changing stored records or granting pool admission.
pub fn select_account_route_group(
    groups: &[AccountRouteGroup],
    current: &str,
    next: &str,
    attach: bool,
) -> Result<AccountRouteGroup, &'static str> {
    let existing = groups
        .iter()
        .find(|group| group.account_ids.iter().any(|id| id == current));
    if existing.is_some_and(|group| group.selected_account_id != current) {
        return Err("The selected account route changed; refresh and try again");
    }
    let mut group = existing.cloned().unwrap_or_else(|| AccountRouteGroup {
        account_ids: vec![current.into()],
        selected_account_id: current.into(),
        notes_account_id: None,
    });
    if attach {
        if groups
            .iter()
            .any(|group| group.account_ids.iter().any(|id| id == next))
            || group.account_ids.iter().any(|id| id == next)
        {
            return Err("This connection is already associated with an account");
        }
        group.account_ids.push(next.into());
    } else if !group.account_ids.iter().any(|id| id == next) {
        return Err("This route does not belong to the selected account");
    }
    group.selected_account_id = next.into();
    Ok(group)
}

pub fn detach_account_route(
    groups: &mut Vec<AccountRouteGroup>,
    account_id: &str,
    connection_id: &str,
) -> Result<(), &'static str> {
    let group = groups
        .iter_mut()
        .find(|group| {
            group.selected_account_id == account_id
                && group.account_ids.iter().any(|id| id == connection_id)
        })
        .ok_or("Account route was not found")?;
    if group.selected_account_id == connection_id {
        return Err("Select another route before detaching this connection");
    }
    group.account_ids.retain(|id| id != connection_id);
    if group.notes_account_id.as_deref() == Some(connection_id) {
        group.notes_account_id = None;
    }
    groups.retain(|group| group.account_ids.len() > 1);
    Ok(())
}

#[cfg(test)]
mod tests;

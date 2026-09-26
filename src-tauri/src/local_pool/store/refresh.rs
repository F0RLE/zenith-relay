//! Desktop observation fences. Revisions are durable, monotonic and secret-free;
//! restoring a previous record never restores permission for an older read.

use super::{serialize_state, LocalPoolStore};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, LocalAccountRecord},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const STATE_REFRESH_REVISIONS: &str = "account_refresh_revisions";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountRefreshFence {
    pub account_id: String,
    revision: u64,
    configuration_revision: u64,
}

impl AccountRefreshFence {
    pub(crate) fn identity(&self) -> zenith_relay_core::scheduler::refresh::RefreshIdentity {
        zenith_relay_core::scheduler::refresh::RefreshIdentity::new(
            format!("account:{}", self.account_id),
            self.revision,
            self.configuration_revision,
        )
    }
}

pub(crate) struct AppliedAccountRefresh<T> {
    pub previous: LocalAccountRecord,
    pub account: LocalAccountRecord,
    pub value: T,
}

#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RefreshRevisions {
    clock: u64,
    configuration: u64,
    accounts: BTreeMap<String, u64>,
}

impl RefreshRevisions {
    pub(super) fn initialize(accounts: &[LocalAccountRecord]) -> Result<Self> {
        Self::default().with_accounts(&[], accounts)
    }

    pub(super) fn validate(&self, accounts: &[LocalAccountRecord]) -> Result<()> {
        if self.configuration > self.clock
            || self.accounts.len() != accounts.len()
            || accounts
                .iter()
                .map(|account| &account.account.id)
                .collect::<BTreeSet<_>>()
                .len()
                != accounts.len()
            || accounts.iter().any(|account| {
                self.accounts
                    .get(&account.account.id)
                    .is_none_or(|revision| *revision == 0 || *revision > self.clock)
            })
        {
            return Err(invalid_revisions());
        }
        Ok(())
    }

    fn next(&mut self) -> Result<u64> {
        self.clock = self.clock.checked_add(1).ok_or_else(invalid_revisions)?;
        Ok(self.clock)
    }

    pub(super) fn with_accounts(
        &self,
        previous: &[LocalAccountRecord],
        accounts: &[LocalAccountRecord],
    ) -> Result<Self> {
        let previous = previous
            .iter()
            .map(|account| (account.account.id.as_str(), account))
            .collect::<BTreeMap<_, _>>();
        let mut next = self.clone();
        next.accounts.clear();
        for account in accounts {
            let id = &account.account.id;
            let revision = if previous
                .get(id.as_str())
                .is_some_and(|old| same_account_scope(old, account))
            {
                self.accounts
                    .get(id)
                    .copied()
                    .ok_or_else(invalid_revisions)?
            } else {
                next.next()?
            };
            if next.accounts.insert(id.clone(), revision).is_some() {
                return Err(invalid_revisions());
            }
        }
        Ok(next)
    }

    pub(super) fn with_gateway(
        &self,
        previous: &GatewaySettings,
        gateway: &GatewaySettings,
    ) -> Result<Self> {
        let mut next = self.clone();
        if previous.common_proxy_configured != gateway.common_proxy_configured
            || previous.account_proxy_required != gateway.account_proxy_required
            || previous.quota_request_timeout_seconds != gateway.quota_request_timeout_seconds
        {
            next.configuration = next.next()?;
        }
        Ok(next)
    }
}

impl LocalPoolStore {
    pub(crate) fn refresh_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.refresh_changed.subscribe()
    }

    pub(crate) fn notify_refresh_changed(&self) {
        self.refresh_changed
            .send_modify(|value| *value = value.wrapping_add(1));
    }

    pub(super) fn refresh_registration_changed(
        &self,
        revisions: &RefreshRevisions,
        accounts: &[LocalAccountRecord],
    ) -> bool {
        if revisions != &self.refresh_revisions {
            return true;
        }
        // Usage/quota/model observations do not trigger inventory-wide scans.
        // Fresh-login eligibility may change without an identity revision.
        self.accounts.len() != accounts.len()
            || self.accounts.iter().zip(accounts).any(|(old, new)| {
                old.account.id != new.account.id
                    || old.account.is_automatic_quota_monitoring_eligible()
                        != new.account.is_automatic_quota_monitoring_eligible()
            })
    }

    pub(crate) fn account_refresh_scope(
        &self,
        account_id: &str,
    ) -> Result<(LocalAccountRecord, AccountRefreshFence)> {
        let account = self
            .account(account_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        let revision = self
            .refresh_revisions
            .accounts
            .get(account_id)
            .copied()
            .ok_or_else(invalid_revisions)?;
        Ok((
            account,
            AccountRefreshFence {
                account_id: account_id.into(),
                revision,
                configuration_revision: self.refresh_revisions.configuration,
            },
        ))
    }

    pub(crate) fn ensure_account_refresh_current(
        &self,
        expected: &AccountRefreshFence,
    ) -> Result<()> {
        if self.refresh_revisions.accounts.get(&expected.account_id) != Some(&expected.revision)
            || self.refresh_revisions.configuration != expected.configuration_revision
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "account changed during refresh",
            ));
        }
        Ok(())
    }

    /// The caller holds DesktopState's store lock across comparison and commit.
    /// The callback reduces only its observation against the latest record.
    pub(crate) fn apply_account_refresh<T>(
        &mut self,
        expected: &AccountRefreshFence,
        apply: impl FnOnce(&mut LocalAccountRecord) -> Result<T>,
    ) -> Result<AppliedAccountRefresh<T>> {
        self.ensure_account_refresh_current(expected)?;
        let previous = self
            .account(&expected.account_id)
            .cloned()
            .ok_or_else(invalid_revisions)?;
        let mut account = previous.clone();
        let value = apply(&mut account)?;
        if account.account.id != previous.account.id || !same_account_scope(&previous, &account) {
            return Err(LocalPoolError::invalid_state(
                "refresh cannot change account configuration",
            ));
        }
        self.upsert_account(account.clone())?;
        Ok(AppliedAccountRefresh {
            previous,
            account,
            value,
        })
    }

    /// Called under setup_guard before a login/import or secret-backed route
    /// change. Even failed/rolled-back mutations retire pre-existing reads.
    /// Ordinary TokenAuthority generation updates do not call this method.
    pub(crate) fn invalidate_account_refresh(&mut self, account_ids: &[&str]) -> Result<()> {
        let mut next = self.refresh_revisions.clone();
        for id in account_ids {
            if next.accounts.contains_key(*id) {
                let revision = next.next()?;
                next.accounts.insert((*id).into(), revision);
            }
        }
        self.persist_refresh_revisions(next)
    }

    /// The common proxy URL is secret-backed, so changing one configured URL
    /// to another must invalidate reads even when the settings boolean is equal.
    pub(crate) fn invalidate_refresh_configuration(&mut self) -> Result<()> {
        let mut next = self.refresh_revisions.clone();
        next.configuration = next.next()?;
        self.persist_refresh_revisions(next)
    }

    fn persist_refresh_revisions(&mut self, next: RefreshRevisions) -> Result<()> {
        if next != self.refresh_revisions {
            self.database
                .replace_state_json(&[(STATE_REFRESH_REVISIONS, serialize_state(&next)?)])?;
            self.refresh_revisions = next;
            self.notify_refresh_changed();
        }
        Ok(())
    }
}

fn same_account_scope(previous: &LocalAccountRecord, account: &LocalAccountRecord) -> bool {
    previous.account.identity == account.account.identity
        && previous.account.auth_mode == account.account.auth_mode
        && previous.account.source_id == account.account.source_id
        && previous.account.secret_refs == account.account.secret_refs
        && previous.account.created_at_ms == account.account.created_at_ms
        && previous.account.enabled == account.account.enabled
        && previous.remote_location == account.remote_location
}

fn invalid_revisions() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "account refresh revisions are invalid",
    )
}

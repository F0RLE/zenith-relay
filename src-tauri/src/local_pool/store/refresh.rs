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
            zenith_relay_core::scheduler::account_member_key(&self.account_id),
            self.revision,
            self.configuration_revision,
        )
    }
}

pub(crate) struct AppliedAccountRefresh<T> {
    pub previous_account: LocalAccountRecord,
    pub account: LocalAccountRecord,
    pub refresh_result: T,
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

    fn allocate_revision(&mut self) -> Result<u64> {
        self.clock = self.clock.checked_add(1).ok_or_else(invalid_revisions)?;
        Ok(self.clock)
    }

    pub(super) fn with_accounts(
        &self,
        previous_accounts: &[LocalAccountRecord],
        accounts: &[LocalAccountRecord],
    ) -> Result<Self> {
        let previous_accounts_by_id = previous_accounts
            .iter()
            .map(|account| (account.account.id.as_str(), account))
            .collect::<BTreeMap<_, _>>();
        let mut updated_revisions = self.clone();
        updated_revisions.accounts.clear();
        for account in accounts {
            let account_id = &account.account.id;
            let revision = if previous_accounts_by_id
                .get(account_id.as_str())
                .is_some_and(|previous_account| same_account_scope(previous_account, account))
            {
                self.accounts
                    .get(account_id)
                    .copied()
                    .ok_or_else(invalid_revisions)?
            } else {
                updated_revisions.allocate_revision()?
            };
            if updated_revisions
                .accounts
                .insert(account_id.clone(), revision)
                .is_some()
            {
                return Err(invalid_revisions());
            }
        }
        Ok(updated_revisions)
    }

    pub(super) fn with_gateway(
        &self,
        previous_gateway: &GatewaySettings,
        gateway: &GatewaySettings,
    ) -> Result<Self> {
        let mut updated_revisions = self.clone();
        if previous_gateway.common_proxy_configured != gateway.common_proxy_configured
            || previous_gateway.account_proxy_required != gateway.account_proxy_required
            || previous_gateway.quota_request_timeout_seconds
                != gateway.quota_request_timeout_seconds
        {
            updated_revisions.configuration = updated_revisions.allocate_revision()?;
        }
        Ok(updated_revisions)
    }
}

impl LocalPoolStore {
    pub(crate) fn refresh_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.refresh_changed.subscribe()
    }

    pub(crate) fn notify_refresh_changed(&self) {
        self.refresh_changed
            .send_modify(|refresh_revision| *refresh_revision = refresh_revision.wrapping_add(1));
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
            || self
                .accounts
                .iter()
                .zip(accounts)
                .any(|(previous_account, current_account)| {
                    previous_account.account.id != current_account.account.id
                        || previous_account
                            .account
                            .is_automatic_quota_monitoring_eligible()
                            != current_account
                                .account
                                .is_automatic_quota_monitoring_eligible()
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
        let previous_account = self
            .account(&expected.account_id)
            .cloned()
            .ok_or_else(invalid_revisions)?;
        let mut account = previous_account.clone();
        let refresh_result = apply(&mut account)?;
        if account.account.id != previous_account.account.id
            || !same_account_scope(&previous_account, &account)
        {
            return Err(LocalPoolError::invalid_state(
                "refresh cannot change account configuration",
            ));
        }
        self.upsert_account(account.clone())?;
        Ok(AppliedAccountRefresh {
            previous_account,
            account,
            refresh_result,
        })
    }

    /// Called under setup_guard before a login/import or secret-backed route
    /// change. Even failed/rolled-back mutations retire pre-existing reads.
    /// Ordinary TokenAuthority generation updates do not call this method.
    pub(crate) fn invalidate_account_refresh(&mut self, account_ids: &[&str]) -> Result<()> {
        let mut updated_revisions = self.refresh_revisions.clone();
        for account_id in account_ids {
            if updated_revisions.accounts.contains_key(*account_id) {
                let revision = updated_revisions.allocate_revision()?;
                updated_revisions
                    .accounts
                    .insert((*account_id).into(), revision);
            }
        }
        self.persist_refresh_revisions(updated_revisions)
    }

    /// The common proxy URL is secret-backed, so changing one configured URL
    /// to another must invalidate reads even when the settings boolean is equal.
    pub(crate) fn invalidate_refresh_configuration(&mut self) -> Result<()> {
        let mut updated_revisions = self.refresh_revisions.clone();
        updated_revisions.configuration = updated_revisions.allocate_revision()?;
        self.persist_refresh_revisions(updated_revisions)
    }

    fn persist_refresh_revisions(&mut self, updated_revisions: RefreshRevisions) -> Result<()> {
        if updated_revisions != self.refresh_revisions {
            self.database.replace_state_json(&[(
                STATE_REFRESH_REVISIONS,
                serialize_state(&updated_revisions)?,
            )])?;
            self.refresh_revisions = updated_revisions;
            self.notify_refresh_changed();
        }
        Ok(())
    }
}

fn same_account_scope(previous_account: &LocalAccountRecord, account: &LocalAccountRecord) -> bool {
    previous_account.account.identity == account.account.identity
        && previous_account.account.auth_mode == account.account.auth_mode
        && previous_account.account.source_id == account.account.source_id
        && previous_account.account.secret_refs == account.account.secret_refs
        && previous_account.account.created_at_ms == account.account.created_at_ms
        && previous_account.account.enabled == account.account.enabled
        && previous_account.remote_location == account.remote_location
}

fn invalid_revisions() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "account refresh revisions are invalid",
    )
}

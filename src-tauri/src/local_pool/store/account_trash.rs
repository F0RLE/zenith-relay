use super::{
    serialize_state, LocalPoolStore, STATE_ACCOUNTS, STATE_ACCOUNT_TRASH, STATE_AUTOMATIONS,
    STATE_REFRESH_REVISIONS,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{AutomationRecords, LocalAccountRecord, TrashedLocalAccount, MAX_LOCAL_ACCOUNTS},
};
use std::collections::BTreeSet;

pub(super) fn validate_inventory(
    accounts: &[LocalAccountRecord],
    trash: &[TrashedLocalAccount],
) -> Result<()> {
    let mut account_ids = BTreeSet::new();
    if accounts
        .iter()
        .map(|account| &account.account.id)
        .chain(trash.iter().map(|entry| &entry.record.account.id))
        .any(|account_id| !account_ids.insert(account_id))
        || accounts.iter().any(|account| {
            trash.iter().any(|entry| {
                entry.record.account.identity.identity_hash
                    == account.account.identity.identity_hash
            })
        })
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "account already exists in the inventory or trash; restore it before importing",
        ));
    }
    Ok(())
}

impl LocalPoolStore {
    pub fn account_trash(&self) -> &[TrashedLocalAccount] {
        &self.account_trash
    }

    pub fn trash_accounts(
        &mut self,
        account_ids: &[String],
        automations: AutomationRecords,
        trashed_at_ms: u64,
    ) -> Result<()> {
        let mut trash = self.account_trash.clone();
        for account_id in account_ids {
            let record = self
                .account(account_id)
                .cloned()
                .ok_or_else(account_not_found)?;
            trash.push(TrashedLocalAccount {
                record,
                trashed_at_ms,
            });
        }
        let accounts = self
            .accounts
            .iter()
            .filter(|account| !account_ids.contains(&account.account.id))
            .cloned()
            .collect();
        self.replace_account_inventory(accounts, trash, automations, &[])
    }

    pub fn restore_trashed_accounts(&mut self, account_ids: &[String]) -> Result<()> {
        let mut accounts = self.accounts.clone();
        for account_id in account_ids {
            let mut record = self
                .account_trash
                .iter()
                .find(|entry| entry.record.account.id == *account_id)
                .map(|entry| entry.record.clone())
                .ok_or_else(account_not_found)?;
            // Restoring inventory must not start requests, monitoring or wake tasks.
            record.account.enabled = false;
            record.account.in_pool = false;
            record.account.draining = false;
            accounts.push(record);
        }
        let trash = self
            .account_trash
            .iter()
            .filter(|entry| !account_ids.contains(&entry.record.account.id))
            .cloned()
            .collect();
        self.replace_account_inventory(accounts, trash, self.automations.clone(), &[])
    }

    pub fn purge_trashed_accounts(&mut self, account_ids: &[String]) -> Result<()> {
        for account_id in account_ids {
            if !self
                .account_trash
                .iter()
                .any(|entry| entry.record.account.id == *account_id)
            {
                return Err(account_not_found());
            }
        }
        let trash = self
            .account_trash
            .iter()
            .filter(|entry| !account_ids.contains(&entry.record.account.id))
            .cloned()
            .collect();
        self.replace_account_inventory(
            self.accounts.clone(),
            trash,
            self.automations.clone(),
            account_ids,
        )
    }

    fn replace_account_inventory(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        trash: Vec<TrashedLocalAccount>,
        automations: AutomationRecords,
        permanently_deleted_ids: &[String],
    ) -> Result<()> {
        validate_inventory(&accounts, &trash)?;
        if accounts.len() > MAX_LOCAL_ACCOUNTS {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "local account limit exceeded",
            ));
        }
        let revisions = self
            .refresh_revisions
            .with_accounts(&self.accounts, &accounts)?;
        let entries = [
            (STATE_ACCOUNTS, serialize_state(&accounts)?),
            (STATE_ACCOUNT_TRASH, serialize_state(&trash)?),
            (STATE_AUTOMATIONS, serialize_state(&automations)?),
            (STATE_REFRESH_REVISIONS, serialize_state(&revisions)?),
        ];
        if permanently_deleted_ids.is_empty() {
            self.database.replace_state_json(&entries)?;
        } else {
            self.database
                .replace_state_json_and_delete_accounts_data(&entries, permanently_deleted_ids)?;
        }
        self.accounts = accounts;
        self.account_trash = trash;
        self.automations = automations;
        self.refresh_revisions = revisions;
        self.notify_refresh_changed();
        Ok(())
    }
}

fn account_not_found() -> LocalPoolError {
    LocalPoolError::new(ErrorCode::NotFound, "account not found")
}

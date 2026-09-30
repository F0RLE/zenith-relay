mod gateway;
mod membership;

use super::{
    persistence, LocalPoolStore, STATE_ACCOUNTS, STATE_AUTOMATIONS, STATE_KEYS,
    STATE_REFRESH_REVISIONS, STATE_SOURCES, STATE_SOURCE_REVISIONS,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{
        AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord,
        MAX_LOCAL_ACCOUNTS,
    },
};

impl LocalPoolStore {
    pub fn replace_records(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
    ) -> Result<()> {
        self.replace_all_records(
            sources,
            self.accounts.clone(),
            keys,
            self.automations.clone(),
        )
    }

    pub fn replace_accounts_and_keys(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
    ) -> Result<()> {
        self.replace_all_records(
            self.sources.clone(),
            accounts,
            keys,
            self.automations.clone(),
        )
    }

    pub fn replace_pool_records(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
    ) -> Result<()> {
        self.replace_all_records(sources, accounts, keys, self.automations.clone())
    }

    pub fn replace_automations(&mut self, automations: AutomationRecords) -> Result<()> {
        self.replace_all_records(
            self.sources.clone(),
            self.accounts.clone(),
            self.keys.clone(),
            automations,
        )
    }

    pub fn replace_account_state(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.replace_all_records(self.sources.clone(), accounts, keys, automations)
    }

    fn replace_all_records(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.replace_all_records_inner(sources, accounts, keys, automations, &[])
    }

    fn replace_all_records_inner(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
        deleted_account_ids: &[String],
    ) -> Result<()> {
        if accounts.len() > MAX_LOCAL_ACCOUNTS {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("local account count exceeds the supported limit of {MAX_LOCAL_ACCOUNTS}"),
            ));
        }
        let changed = persistence::RecordChanges {
            sources: sources != self.sources,
            accounts: accounts != self.accounts,
            keys: keys != self.keys,
            automations: automations != self.automations,
        };
        if !changed.any() {
            return Ok(());
        }

        let revisions = if changed.accounts {
            self.refresh_revisions
                .with_accounts(&self.accounts, &accounts)?
        } else {
            self.refresh_revisions.clone()
        };
        let source_revisions = self
            .source_refresh_revisions
            .with_sources(&self.sources, &sources)?;
        let mut values = Vec::with_capacity(6);
        if source_revisions != self.source_refresh_revisions {
            values.push((
                STATE_SOURCE_REVISIONS,
                persistence::serialize_state(&source_revisions)?,
            ));
        }
        if revisions != self.refresh_revisions {
            values.push((
                STATE_REFRESH_REVISIONS,
                persistence::serialize_state(&revisions)?,
            ));
        }
        if changed.sources {
            values.push((STATE_SOURCES, persistence::serialize_state(&sources)?));
        }
        if changed.accounts {
            values.push((STATE_ACCOUNTS, persistence::serialize_state(&accounts)?));
        }
        if changed.keys {
            values.push((STATE_KEYS, persistence::serialize_state(&keys)?));
        }
        if changed.automations {
            values.push((
                STATE_AUTOMATIONS,
                persistence::serialize_state(&automations)?,
            ));
        }
        if deleted_account_ids.is_empty() {
            self.database.replace_state_json(&values)?;
        } else if deleted_account_ids.len() == 1 {
            self.database
                .replace_state_json_and_delete_account_data(&values, &deleted_account_ids[0])?;
        } else {
            self.database
                .replace_state_json_and_delete_accounts_data(&values, deleted_account_ids)?;
        }

        let refresh_changed = self.refresh_registration_changed(&revisions, &accounts)
            || source_revisions != self.source_refresh_revisions;
        self.source_refresh_revisions = source_revisions;
        self.sources = sources;
        self.accounts = accounts;
        self.keys = keys;
        self.automations = automations;
        self.refresh_revisions = revisions;
        if refresh_changed {
            self.notify_refresh_changed();
        }
        Ok(())
    }
}

mod gateway;
mod membership;
mod usage;

use std::borrow::Cow;

use super::{
    persistence, LocalPoolStore, STATE_ACCOUNTS, STATE_AUTOMATIONS, STATE_GATEWAY, STATE_KEYS,
    STATE_REFRESH_REVISIONS, STATE_SOURCES, STATE_SOURCE_REVISIONS,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{
        AutomationRecords, GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord,
        ProviderSourceRecord, MAX_LOCAL_ACCOUNTS,
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

    #[cfg(test)]
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
        self.replace_all_records_inner(sources, accounts, keys, automations, &[], None)
    }

    pub fn replace_account_routes(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        gateway: GatewaySettings,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.replace_all_records_inner(
            self.sources.clone(),
            accounts,
            self.keys.clone(),
            automations,
            &[],
            Some(gateway),
        )
    }

    fn replace_all_records_inner(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
        deleted_account_ids: &[String],
        gateway: Option<GatewaySettings>,
    ) -> Result<()> {
        let explicit_gateway = gateway.is_some();
        let mut gateway = gateway.map_or(Cow::Borrowed(&self.gateway), Cow::Owned);
        if !gateway.account_routes.is_empty()
            && (explicit_gateway
                || accounts
                    .iter()
                    .map(|account| &account.account.id)
                    .ne(self.accounts.iter().map(|account| &account.account.id)))
        {
            gateway.to_mut().retain_account_routes(&accounts);
        }
        let gateway_changed = matches!(&gateway, Cow::Owned(next) if next != &self.gateway);
        if gateway_changed {
            gateway.validate().map_err(LocalPoolError::invalid_state)?;
        }
        if !gateway.account_routes.is_empty()
            && accounts.iter().any(|account| {
                account.account.in_pool && !gateway.account_route_is_selected(&account.account.id)
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "Only the selected account route can participate in the pool",
            ));
        }
        super::account_trash::validate_inventory(&accounts, &self.account_trash)?;
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
        if !changed.any() && !gateway_changed {
            return Ok(());
        }

        let mut revisions = if changed.accounts {
            self.refresh_revisions
                .with_accounts(&self.accounts, &accounts)?
        } else {
            self.refresh_revisions.clone()
        };
        if gateway_changed {
            revisions = revisions.with_gateway(&self.gateway, &gateway)?;
        }
        let source_revisions = self
            .source_refresh_revisions
            .with_sources(&self.sources, &sources)?;
        let mut state_entries = Vec::with_capacity(6);
        if gateway_changed {
            state_entries.push((
                STATE_GATEWAY,
                persistence::serialize_state(gateway.as_ref())?,
            ));
        }
        if source_revisions != self.source_refresh_revisions {
            state_entries.push((
                STATE_SOURCE_REVISIONS,
                persistence::serialize_state(&source_revisions)?,
            ));
        }
        if revisions != self.refresh_revisions {
            state_entries.push((
                STATE_REFRESH_REVISIONS,
                persistence::serialize_state(&revisions)?,
            ));
        }
        if changed.sources {
            state_entries.push((STATE_SOURCES, persistence::serialize_state(&sources)?));
        }
        if changed.accounts {
            state_entries.push((STATE_ACCOUNTS, persistence::serialize_state(&accounts)?));
        }
        if changed.keys {
            state_entries.push((STATE_KEYS, persistence::serialize_state(&keys)?));
        }
        if changed.automations {
            state_entries.push((
                STATE_AUTOMATIONS,
                persistence::serialize_state(&automations)?,
            ));
        }
        if deleted_account_ids.is_empty() {
            self.database.replace_state_json(&state_entries)?;
        } else if deleted_account_ids.len() == 1 {
            self.database.replace_state_json_and_delete_account_data(
                &state_entries,
                &deleted_account_ids[0],
            )?;
        } else {
            self.database
                .replace_state_json_and_delete_accounts_data(&state_entries, deleted_account_ids)?;
        }

        let refresh_changed = self.refresh_registration_changed(&revisions, &accounts)
            || source_revisions != self.source_refresh_revisions;
        self.source_refresh_revisions = source_revisions;
        self.sources = sources;
        self.accounts = accounts;
        if let Cow::Owned(gateway) = gateway {
            self.gateway = gateway;
        }
        self.keys = keys;
        self.automations = automations;
        self.refresh_revisions = revisions;
        if refresh_changed {
            self.notify_refresh_changed();
        }
        Ok(())
    }
}

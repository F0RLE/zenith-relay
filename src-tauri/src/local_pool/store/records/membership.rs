use super::super::LocalPoolStore;
use crate::local_pool::error::Result;
use crate::local_pool::models::{
    AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord,
};

impl LocalPoolStore {
    pub fn upsert_source(&mut self, source: ProviderSourceRecord) -> Result<()> {
        let mut next = self.sources.clone();
        if let Some(current) = next.iter_mut().find(|current| current.id == source.id) {
            *current = source;
        } else {
            next.push(source);
        }
        self.replace_records(next, self.keys.clone())
    }

    pub fn upsert_key(&mut self, key: LocalGatewayKeyRecord) -> Result<()> {
        let mut next = self.keys.clone();
        if let Some(current) = next.iter_mut().find(|current| current.id == key.id) {
            *current = key;
        } else {
            next.push(key);
        }
        self.replace_records(self.sources.clone(), next)
    }

    pub fn upsert_account(&mut self, account: LocalAccountRecord) -> Result<()> {
        let mut next = self.accounts.clone();
        if let Some(current) = next
            .iter_mut()
            .find(|current| current.account.id == account.account.id)
        {
            *current = account;
        } else {
            next.push(account);
        }
        self.replace_accounts_and_keys(next, self.keys.clone())
    }

    /// Restores one account only when its current record still belongs to the
    /// transaction that is being rolled back. This avoids a failed runtime
    /// rebuild replacing the entire account snapshot and erasing a concurrent
    /// login, quota observation, or edit of another account.
    ///
    /// A client-login watchdog observation is presentation-only, so preserve a
    /// newer observation while restoring the transaction's previous record.
    pub fn restore_account_if_current(
        &mut self,
        previous: &LocalAccountRecord,
        attempted: &LocalAccountRecord,
    ) -> Result<bool> {
        let Some(current) = self.account(&attempted.account.id).cloned() else {
            return Ok(false);
        };
        if !current.matches_rollback_snapshot(attempted) {
            return Ok(false);
        }
        let mut restored = previous.clone();
        restored.client_auth_status = current.client_auth_status;
        restored.last_client_login_redirect_at_ms = current.last_client_login_redirect_at_ms;
        self.upsert_account(restored)?;
        Ok(true)
    }

    /// Persists a Team breaker fan-out without synthesizing a usage event.
    /// This updates only the listed local accounts and keeps telemetry intact.
    pub fn block_accounts_for_team(&mut self, account_ids: &[String]) -> Result<bool> {
        let mut changed = false;
        let mut accounts = self.accounts.clone();
        for account in &mut accounts {
            if account_ids.iter().any(|id| id == &account.account.id)
                && (account.account.health
                    != zenith_relay_core::accounts::AccountHealthState::Blocked
                    || account.account.last_error_code.as_deref() != Some("deactivated_workspace"))
            {
                account.account.health = zenith_relay_core::accounts::AccountHealthState::Blocked;
                account.account.last_error_code = Some("deactivated_workspace".to_string());
                changed = true;
            }
        }
        if changed {
            self.replace_all_records(
                self.sources.clone(),
                accounts,
                self.keys.clone(),
                self.automations.clone(),
            )?;
        }
        Ok(changed)
    }

    pub fn delete_account_state(
        &mut self,
        account_id: &str,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.delete_accounts_state(
            std::slice::from_ref(&account_id.to_string()),
            accounts,
            keys,
            automations,
        )
    }

    pub fn delete_accounts_state(
        &mut self,
        account_ids: &[String],
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.replace_all_records_inner(
            self.sources.clone(),
            accounts,
            keys,
            automations,
            account_ids,
        )
    }

    pub fn touch_usage(
        &mut self,
        key_id: &str,
        source_id: &str,
        account_id: Option<&str>,
        at: String,
    ) -> Result<()> {
        let mut sources = self.sources.clone();
        let mut keys = self.keys.clone();
        if let Some(source) = sources.iter_mut().find(|source| source.id == source_id) {
            source.last_used_at = Some(at.clone());
        }
        if let Some(key) = keys.iter_mut().find(|key| key.id == key_id) {
            key.last_used_at = Some(at.clone());
        }
        let mut accounts = self.accounts.clone();
        if let (Some(account_id), Some(last_used_at_ms)) = (
            account_id,
            zenith_relay_core::unix_time_ms_from_rfc3339(&at),
        ) {
            if let Some(account) = accounts
                .iter_mut()
                .find(|account| account.account.id == account_id)
            {
                account.account.last_used_at_ms = Some(last_used_at_ms);
            }
        }
        self.replace_all_records(sources, accounts, keys, self.automations.clone())
    }
}

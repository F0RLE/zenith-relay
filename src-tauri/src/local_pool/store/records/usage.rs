use super::*;
use crate::local_pool::store::refresh::same_account_scope;

fn is_later_usage(previous: Option<&str>, incoming: &str) -> bool {
    let Ok(incoming) = chrono::DateTime::parse_from_rfc3339(incoming) else {
        return false;
    };
    previous
        .and_then(|previous| chrono::DateTime::parse_from_rfc3339(previous).ok())
        .is_none_or(|previous| incoming > previous)
}

impl LocalPoolStore {
    fn touched_key(&self, key_id: &str, at: &str) -> Option<(usize, LocalGatewayKeyRecord)> {
        self.keys
            .iter()
            .enumerate()
            .find(|(_, key)| key.id == key_id)
            .filter(|(_, key)| is_later_usage(key.last_used_at.as_deref(), at))
            .map(|(index, key)| {
                let mut key = key.clone();
                key.last_used_at = Some(at.to_string());
                (index, key)
            })
    }

    pub(crate) fn touch_usage(&mut self, key_id: &str, source_id: &str, at: &str) -> Result<()> {
        let source = self
            .sources
            .iter()
            .enumerate()
            .find(|(_, source)| source.id == source_id)
            .filter(|(_, source)| is_later_usage(source.last_used_at.as_deref(), at))
            .map(|(index, source)| {
                let mut source = source.clone();
                source.last_used_at = Some(at.to_string());
                (index, source)
            });
        let key = self.touched_key(key_id, at);
        let mut items = Vec::with_capacity(2);
        if let Some((index, source)) = &source {
            items.push((STATE_SOURCES, *index, persistence::serialize_state(source)?));
        }
        if let Some((index, key)) = &key {
            items.push((STATE_KEYS, *index, persistence::serialize_state(key)?));
        }
        self.database.replace_state_items(&items, &[])?;
        // Publish only after durable commit; no inventory-wide clones or writes.
        if let Some((index, source)) = source {
            self.sources[index] = source;
        }
        if let Some((index, key)) = key {
            self.keys[index] = key;
        }
        Ok(())
    }

    pub(crate) fn persist_account_usage(
        &mut self,
        account: LocalAccountRecord,
        key_id: &str,
        at: &str,
        automations: AutomationRecords,
    ) -> Result<()> {
        let index = self
            .accounts
            .iter()
            .position(|previous| previous.account.id == account.account.id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
        let previous = &self.accounts[index];
        // This fast path is only for observations. Identity/configuration
        // replacement must use the existing revision-fenced inventory path.
        if !same_account_scope(previous, &account) {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "usage cannot change account registration",
            ));
        }
        let eligibility_changed = previous.account.is_automatic_quota_monitoring_eligible()
            != account.account.is_automatic_quota_monitoring_eligible();
        let key = self.touched_key(key_id, at);
        let mut items = Vec::with_capacity(2);
        if previous != &account {
            items.push((
                STATE_ACCOUNTS,
                index,
                persistence::serialize_state(&account)?,
            ));
        }
        if let Some((index, key)) = &key {
            items.push((STATE_KEYS, *index, persistence::serialize_state(key)?));
        }
        let entries = if automations != self.automations {
            vec![(
                STATE_AUTOMATIONS,
                persistence::serialize_state(&automations)?,
            )]
        } else {
            Vec::new()
        };
        self.database.replace_state_items(&items, &entries)?;
        self.accounts[index] = account;
        if let Some((index, key)) = key {
            self.keys[index] = key;
        }
        self.automations = automations;
        if eligibility_changed {
            self.notify_refresh_changed();
        }
        Ok(())
    }
}

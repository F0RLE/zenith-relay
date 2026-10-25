use super::{
    db_error, lock_error, ErrorCode, LocalPoolError, Result, TelemetryDb, MAX_STATE_JSON_BYTES,
};
use rusqlite::OptionalExtension;
use rusqlite::{params, TransactionBehavior};
use std::collections::HashMap;

mod items;

impl TelemetryDb {
    pub(crate) fn state_json_entries(&self) -> Result<HashMap<String, String>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection
            .prepare("SELECT key, value_json FROM app_state")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db_error)?;
        let mut entries = rows
            .map(|row| row.map_err(db_error))
            .collect::<std::result::Result<HashMap<_, _>, _>>()?;
        items::overlay(&connection, &mut entries)?;
        Ok(entries)
    }

    pub(crate) fn state_json(&self, key: &str) -> Result<Option<String>> {
        validate_state_key(key)?;
        let connection = self.connection.lock().map_err(lock_error)?;
        let value = connection
            .query_row(
                "SELECT value_json FROM app_state WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let Some(value) = value else {
            return Ok(None);
        };
        let mut entries = HashMap::from([(key.to_string(), value)]);
        items::overlay(&connection, &mut entries)?;
        Ok(entries.remove(key))
    }

    #[cfg(test)]
    pub(crate) fn state_count(&self) -> Result<usize> {
        let count: i64 = self
            .connection
            .lock()
            .map_err(lock_error)?
            .query_row("SELECT COUNT(*) FROM app_state", [], |row| row.get(0))
            .map_err(db_error)?;
        usize::try_from(count).map_err(|_| {
            LocalPoolError::new(ErrorCode::RecoveryRequired, "local state count is invalid")
        })
    }

    pub(crate) fn replace_state_json(&self, state_entries: &[(&str, String)]) -> Result<()> {
        self.replace_state_json_with_account_purge(state_entries, &[], &[])
    }

    pub(crate) fn replace_state_items(
        &self,
        items: &[(&str, usize, String)],
        state_entries: &[(&str, String)],
    ) -> Result<()> {
        self.replace_state_json_with_account_purge(state_entries, &[], items)
    }

    pub(crate) fn replace_state_json_and_delete_account_data(
        &self,
        state_entries: &[(&str, String)],
        account_id: &str,
    ) -> Result<()> {
        let account_ids = vec![account_id.to_string()];
        self.replace_state_json_and_delete_accounts_data(state_entries, &account_ids)
    }

    pub(crate) fn replace_state_json_and_delete_accounts_data(
        &self,
        state_entries: &[(&str, String)],
        account_ids: &[String],
    ) -> Result<()> {
        let account_ids = account_ids.iter().map(String::as_str).collect::<Vec<_>>();
        self.replace_state_json_with_account_purge(state_entries, &account_ids, &[])
    }

    fn replace_state_json_with_account_purge(
        &self,
        state_entries: &[(&str, String)],
        account_ids: &[&str],
        item_entries: &[(&str, usize, String)],
    ) -> Result<()> {
        if state_entries.is_empty() && account_ids.is_empty() && item_entries.is_empty() {
            return Ok(());
        }
        for (key, state_json) in state_entries {
            validate_state_key(key)?;
            if state_json.len() > MAX_STATE_JSON_BYTES {
                return Err(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "local state value is too large",
                ));
            }
        }
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if !state_entries.is_empty() {
            let mut statement = transaction
                .prepare_cached(
                    "INSERT INTO app_state(key, value_json) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json
                     WHERE app_state.value_json IS NOT excluded.value_json",
                )
                .map_err(db_error)?;
            for (key, state_json) in state_entries {
                statement
                    .execute(params![key, state_json])
                    .map_err(db_error)?;
                // The supplied full inventory already includes its observations.
                // Clearing atomically prevents old positional rows resurrecting
                // a removed account or overwriting reordered/reimported records.
                transaction
                    .execute("DELETE FROM app_state_items WHERE state_key = ?1", [key])
                    .map_err(db_error)?;
            }
        }
        items::write(&transaction, item_entries)?;
        for account_id in account_ids {
            // Shared-ledger estimates cannot be decomposed after a member's
            // underlying usage is deleted. Remove that derived evidence too.
            for table in ["weekly_capacity_anchors", "weekly_capacity_history"] {
                transaction.execute(
                    &format!("DELETE FROM {table} WHERE EXISTS (SELECT 1 FROM json_each(payload_json, '$.account_ids') WHERE value = ?1)"),
                    [account_id],
                ).map_err(db_error)?;
            }
            transaction
                .execute(
                    "DELETE FROM request_attempts WHERE account_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM request_logs WHERE account_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    zenith_relay_core::usage::DELETE_ACCOUNT_CANDIDATE_ROLLUPS_SQL,
                    [account_id],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM response_affinity WHERE candidate_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        if !account_ids.is_empty() {
            self.capacity_seen.lock().map_err(lock_error)?.clear();
            self.capacity_revision
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            self.clear_cached_usage_totals()?;
            self.invalidate_usage_cache();
        }
        Ok(())
    }
}

fn validate_state_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "local state key is invalid",
        ));
    }
    Ok(())
}

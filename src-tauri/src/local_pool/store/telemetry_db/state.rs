use super::{
    db_error, lock_error, ErrorCode, LocalPoolError, Result, TelemetryDb, MAX_STATE_JSON_BYTES,
};
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{params, TransactionBehavior};
use std::collections::HashMap;

impl TelemetryDb {
    pub(crate) fn state_json_values(&self) -> Result<HashMap<String, String>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection
            .prepare("SELECT key, value_json FROM app_state")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db_error)?;
        rows.map(|row| row.map_err(db_error))
            .collect::<std::result::Result<HashMap<_, _>, _>>()
    }

    #[cfg(test)]
    pub(crate) fn state_json(&self, key: &str) -> Result<Option<String>> {
        validate_state_key(key)?;
        self.connection
            .lock()
            .map_err(lock_error)?
            .query_row(
                "SELECT value_json FROM app_state WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)
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

    pub(crate) fn replace_state_json(&self, values: &[(&str, String)]) -> Result<()> {
        self.replace_state_json_with_account_purge(values, &[])
    }

    pub(crate) fn replace_state_json_and_delete_account_data(
        &self,
        values: &[(&str, String)],
        account_id: &str,
    ) -> Result<()> {
        let account_ids = vec![account_id.to_string()];
        self.replace_state_json_and_delete_accounts_data(values, &account_ids)
    }

    pub(crate) fn replace_state_json_and_delete_accounts_data(
        &self,
        values: &[(&str, String)],
        account_ids: &[String],
    ) -> Result<()> {
        let account_ids = account_ids.iter().map(String::as_str).collect::<Vec<_>>();
        self.replace_state_json_with_account_purge(values, &account_ids)
    }

    fn replace_state_json_with_account_purge(
        &self,
        values: &[(&str, String)],
        account_ids: &[&str],
    ) -> Result<()> {
        for (key, value) in values {
            validate_state_key(key)?;
            if value.len() > MAX_STATE_JSON_BYTES {
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
        for (key, value) in values {
            transaction
                .execute(
                    "INSERT INTO app_state(key, value_json) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                    params![key, value],
                )
                .map_err(db_error)?;
        }
        for account_id in account_ids {
            transaction
                .execute(
                    "DELETE FROM request_logs WHERE account_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id = ?1",
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

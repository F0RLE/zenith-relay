use super::sqlite::{db_error, parse_json, to_json, Store};
use crate::state::ServerAccountRecord;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use zenith_relay_core::protocol::TrashedAccountSummary;

impl Store {
    pub(crate) fn trashed_accounts(&self) -> Result<Vec<ServerAccountRecord>, String> {
        self.list_records("account_trash")
    }

    pub fn account_trash(&self) -> Result<Vec<TrashedAccountSummary>, String> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, json_extract(data_json, '$.label'), trashed_at_ms FROM account_trash ORDER BY trashed_at_ms DESC, id",
        ).map_err(db_error)?;
        let entries = statement
            .query_map([], |row| {
                Ok(TrashedAccountSummary {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    trashed_at_ms: row.get::<_, i64>(2)?.max(0) as u64,
                })
            })
            .map_err(db_error)?;
        entries.collect::<Result<Vec<_>, _>>().map_err(db_error)
    }

    pub fn trashed_account(&self, account_id: &str) -> Result<Option<ServerAccountRecord>, String> {
        self.lock()?
            .query_row(
                "SELECT data_json FROM account_trash WHERE id = ?1",
                [account_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?
            .map(|json| parse_json(&json))
            .transpose()
    }

    pub fn trash_account(&self, account_id: &str, trashed_at_ms: u64) -> Result<bool, String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let moved = transaction
            .execute(
                "INSERT INTO account_trash(id, data_json, secret_ref, trashed_at_ms)
             SELECT id, data_json, secret_ref, ?2 FROM accounts WHERE id = ?1",
                params![account_id, zenith_relay_core::usage::sql_u64(trashed_at_ms)],
            )
            .map_err(db_error)?
            > 0;
        if moved {
            transaction
                .execute("DELETE FROM accounts WHERE id = ?1", [account_id])
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM response_affinity WHERE candidate_id = ?1",
                    [account_id],
                )
                .map_err(db_error)?;
            super::automations::prune_account_wake_tasks(&transaction, account_id, trashed_at_ms)?;
        }
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(moved)
    }

    pub fn restore_trashed_account(&self, account_id: &str) -> Result<bool, String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let json = transaction
            .query_row(
                "SELECT data_json FROM account_trash WHERE id = ?1",
                [account_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?;
        let Some(json) = json else {
            return Ok(false);
        };
        let mut record: ServerAccountRecord = parse_json(&json)?;
        record.enabled = false;
        record.in_pool = false;
        record.draining = false;
        // A saved proxy assignment may now belong to another account.
        record.proxy_id = None;
        transaction
            .execute("DELETE FROM account_trash WHERE id = ?1", [account_id])
            .map_err(db_error)?;
        transaction
            .execute(
                "INSERT INTO accounts(id, data_json, secret_ref) VALUES (?1, ?2, ?3)",
                params![record.id, to_json(&record)?, record.secret_ref],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(true)
    }
}

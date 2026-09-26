//! Transactional account observation application. A late read may change only
//! its owned fields in the latest record, never a captured configuration blob.

use super::{
    sqlite::{db_error, parse_json, to_json},
    Store,
};
use crate::state::ServerAccountRecord;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use zenith_relay_core::scheduler::refresh::RefreshIdentity;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountRefreshFence {
    pub account_id: String,
    revision: u64,
    configuration_revision: u64,
}

impl AccountRefreshFence {
    pub fn identity(&self) -> RefreshIdentity {
        RefreshIdentity::new(
            format!("account:{}", self.account_id),
            self.revision,
            self.configuration_revision,
        )
    }
}

impl Store {
    pub(crate) fn refresh_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.refresh_changed.subscribe()
    }

    pub(crate) fn notify_refresh_changed(&self) {
        self.refresh_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub(crate) fn account_refresh_scope(
        &self,
        id: &str,
    ) -> Result<(ServerAccountRecord, AccountRefreshFence), String> {
        let connection = self.lock()?;
        read_scope(&connection, id)
    }

    /// Capture records and refresh revisions under the same database lock.
    /// A late snapshot must not attach a replacement login's scope to old data.
    pub(crate) fn account_refresh_scopes(
        &self,
    ) -> Result<Vec<(ServerAccountRecord, AccountRefreshFence)>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT id, data_json, refresh_revision, (SELECT CAST(value AS INTEGER) FROM metadata WHERE key = 'refresh_config_revision') FROM accounts ORDER BY id")
            .map_err(db_error)?;
        let scopes = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(db_error)?
            .map(|row| {
                let (id, json, revision, configuration_revision) = row.map_err(db_error)?;
                Ok((
                    parse_json(&json)?,
                    AccountRefreshFence {
                        account_id: id,
                        revision: u64::try_from(revision)
                            .map_err(|_| "account refresh revision is invalid")?,
                        configuration_revision: u64::try_from(configuration_revision)
                            .map_err(|_| "refresh configuration revision is invalid")?,
                    },
                ))
            })
            .collect();
        scopes
    }

    pub(crate) fn apply_account_refresh<T>(
        &self,
        expected: &AccountRefreshFence,
        apply: impl FnOnce(&mut ServerAccountRecord) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let (mut account, current) = read_scope(&transaction, &expected.account_id)?;
        if &current != expected {
            return Err("account changed during refresh".into());
        }
        let result = apply(&mut account)?;
        transaction
            .execute(
                "UPDATE accounts SET data_json = ?1 WHERE id = ?2",
                params![to_json(&account)?, account.id],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        drop(connection);
        self.notify_refresh_changed();
        Ok(result)
    }
}

fn read_scope(
    connection: &Connection,
    id: &str,
) -> Result<(ServerAccountRecord, AccountRefreshFence), String> {
    let (json, revision, configuration_revision) = connection.query_row(
        "SELECT data_json, refresh_revision, (SELECT CAST(value AS INTEGER) FROM metadata WHERE key = 'refresh_config_revision') FROM accounts WHERE id = ?1",
        [id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)))
        .optional().map_err(db_error)?.ok_or_else(|| "account not found".to_string())?;
    let revision = u64::try_from(revision).map_err(|_| "account refresh revision is invalid")?;
    let configuration_revision = u64::try_from(configuration_revision)
        .map_err(|_| "refresh configuration revision is invalid")?;
    Ok((
        parse_json(&json)?,
        AccountRefreshFence {
            account_id: id.into(),
            revision,
            configuration_revision,
        },
    ))
}

#[cfg(test)]
mod tests;

//! Source observations merge into current configuration under a durable fence.
//! Normal saves (including batches/presets) are fenced by database triggers;
//! credential replacement explicitly invalidates even if the bytes are equal.
use super::{
    sqlite::{db_error, parse_json, to_json},
    Store,
};
use crate::state::SourceRecord;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use zenith_relay_core::scheduler::refresh::RefreshIdentity;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceRefreshFence {
    pub source_id: String,
    revision: u64,
}

impl SourceRefreshFence {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn identity(&self) -> RefreshIdentity {
        RefreshIdentity::new(format!("source:{}", self.source_id), self.revision, 0)
    }
}

impl Store {
    /// Capture records and their revisions together. A separate revisions query
    /// could incorrectly label an old record with a new credential incarnation.
    pub(crate) fn source_refresh_scopes(
        &self,
    ) -> Result<Vec<(SourceRecord, SourceRefreshFence)>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT id, data_json, refresh_revision FROM sources ORDER BY id")
            .map_err(db_error)?;
        let revisions = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(db_error)?
            .map(|row| {
                let (id, json, revision) = row.map_err(db_error)?;
                let revision = u64::try_from(revision)
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| "source refresh revision is invalid".to_string())?;
                Ok((
                    parse_json(&json)?,
                    SourceRefreshFence {
                        source_id: id,
                        revision,
                    },
                ))
            })
            .collect();
        revisions
    }

    pub(crate) fn source_refresh_scope(
        &self,
        id: &str,
    ) -> Result<(SourceRecord, SourceRefreshFence), String> {
        let connection = self.lock()?;
        read_scope(&connection, id)
    }

    pub(crate) fn invalidate_source_refresh(&self, id: &str) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE refresh_revision_clock SET revision = revision + 1 WHERE id = 1",
                [],
            )
            .map_err(db_error)?;
        transaction.execute("UPDATE sources SET refresh_revision = (SELECT revision FROM refresh_revision_clock WHERE id = 1) WHERE id = ?1", [id]).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        drop(connection);
        self.notify_refresh_changed();
        Ok(())
    }

    pub(crate) fn apply_source_refresh(
        &self,
        fence: &SourceRefreshFence,
        apply: impl FnOnce(&mut SourceRecord) -> Result<(), String>,
    ) -> Result<SourceRecord, String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let (mut record, current) = read_scope(&transaction, &fence.source_id)?;
        if &current != fence {
            return Err("source changed during refresh".into());
        }
        let before = record.clone();
        apply(&mut record)?;
        if record.id != before.id
            || record.secret_ref != before.secret_ref
            || record.enabled != before.enabled
        {
            return Err("source observation cannot change identity or eligibility".into());
        }
        transaction.execute("UPDATE sources SET data_json = ?1, observation_sequence = observation_sequence + 1 WHERE id = ?2", params![to_json(&record)?, record.id]).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        drop(connection);
        self.notify_refresh_changed();
        Ok(record)
    }
}

fn read_scope(
    connection: &Connection,
    id: &str,
) -> Result<(SourceRecord, SourceRefreshFence), String> {
    let (json, revision) = connection
        .query_row(
            "SELECT data_json, refresh_revision FROM sources WHERE id = ?1",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| "source not found".to_string())?;
    let revision = u64::try_from(revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| "source refresh revision is invalid".to_string())?;
    Ok((
        parse_json(&json)?,
        SourceRefreshFence {
            source_id: id.into(),
            revision,
        },
    ))
}

#[cfg(test)]
mod tests;

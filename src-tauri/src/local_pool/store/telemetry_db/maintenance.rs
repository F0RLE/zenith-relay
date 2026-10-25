use super::*;

impl TelemetryDb {
    /// One bounded transaction, never queue maintenance behind a live writer.
    /// Repeated calls drain an old backlog without a long startup transaction.
    pub(crate) fn maintain_usage(&self) -> Result<u64> {
        let mut connection = match self.connection.try_lock() {
            Ok(connection) => connection,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(0),
            Err(std::sync::TryLockError::Poisoned(error)) => return Err(lock_error(error)),
        };
        let transaction = connection.transaction().map_err(db_error)?;
        transaction
            .execute_batch(ARCHIVE_USAGE_SQL)
            .map_err(db_error)?;
        // The final statement clears the exact captured batch.
        let removed = transaction.changes();
        transaction.commit().map_err(db_error)?;
        if removed > 0 {
            self.clear_cached_usage_totals()?;
        }
        drop(connection);
        if removed > 0 {
            self.invalidate_usage_cache();
        }
        Ok(removed)
    }
}

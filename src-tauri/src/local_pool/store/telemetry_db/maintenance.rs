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
        let capacity_removed = transaction.execute(
            "DELETE FROM weekly_capacity_history WHERE observed_at_ms < (unixepoch('now') - 30 * 86400) * 1000", [],
        ).map_err(db_error)? as u64;
        transaction.execute(
            "DELETE FROM weekly_capacity_anchors WHERE observed_at_ms < (unixepoch('now') - 3 * 86400) * 1000", [],
        ).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        if capacity_removed > 0 {
            self.capacity_revision
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
        if removed > 0 {
            self.clear_cached_usage_totals()?;
            self.invalidate_usage_cache();
        }
        Ok(removed + capacity_removed)
    }
}

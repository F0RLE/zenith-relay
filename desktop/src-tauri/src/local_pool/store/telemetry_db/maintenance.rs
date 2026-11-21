use super::*;
use std::{sync::atomic::Ordering, time::Duration};

const MAINTENANCE_RETRY: Duration = Duration::from_secs(60);
const MAX_MAINTENANCE_IDLE: Duration = Duration::from_secs(60 * 60);

impl TelemetryDb {
    pub(crate) fn maintenance_revision(&self) -> (u64, u64) {
        (
            self.usage_revision.load(Ordering::Acquire),
            self.capacity_revision.load(Ordering::Acquire),
        )
    }

    /// Indexed retention deadlines; new usage/capacity revisions wake the host sooner.
    pub(crate) fn next_maintenance_delay(&self) -> Result<Duration> {
        let connection = match self.connection.try_lock() {
            Ok(connection) => connection,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(MAINTENANCE_RETRY),
            Err(std::sync::TryLockError::Poisoned(error)) => return Err(lock_error(error)),
        };
        let mut earliest: Option<i64> = connection.query_row(
            "SELECT MIN(deadline) FROM (
                SELECT (SELECT unixepoch(created_at) * 1000 + 30 * 86400000 FROM request_logs ORDER BY created_at, id LIMIT 1) AS deadline
                UNION ALL SELECT MIN(observed_at_ms) + 30 * 86400000 FROM weekly_capacity_history
                UNION ALL SELECT MIN(observed_at_ms) + 3 * 86400000 FROM weekly_capacity_anchors
             )", [], |row| row.get(0),
        ).map_err(db_error)?;
        for table in ["request_logs", "request_attempts"] {
            let inline: bool = connection.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE routing_json IS NOT NULL OR tool_use_json IS NOT NULL)"),
                [], |row| row.get(0),
            ).map_err(db_error)?;
            if inline {
                return Ok(MAINTENANCE_RETRY);
            }
            let deadline: Option<i64> = connection.query_row(
                &format!("SELECT (SELECT unixepoch(created_at) * 1000 + 7 * 86400000 FROM {table}
                    WHERE diagnostics_pruned = 0 AND (routing_diagnostics_id IS NOT NULL OR tool_diagnostics_id IS NOT NULL)
                    ORDER BY created_at, id LIMIT 1)"), [], |row| row.get(0),
            ).map_err(db_error)?;
            earliest = earliest.into_iter().chain(deadline).min();
        }
        let now = zenith_relay_core::unix_time_ms();
        Ok(earliest.map_or(MAX_MAINTENANCE_IDLE, |deadline| {
            Duration::from_millis(u64::try_from(deadline).unwrap_or(0).saturating_sub(now))
                .clamp(MAINTENANCE_RETRY, MAX_MAINTENANCE_IDLE)
        }))
    }

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
        let compacted = super::diagnostics::compact(&transaction)?;
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
        if removed > 0 || compacted > 0 {
            self.clear_cached_usage_totals()?;
            self.invalidate_usage_cache();
        }
        Ok(removed + capacity_removed + compacted)
    }
}

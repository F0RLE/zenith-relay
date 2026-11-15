use super::*;
use std::{ops::Deref, sync::MutexGuard, sync::atomic::Ordering};

/// One bounded reader with a stable SQL snapshot of committed data.
pub(super) struct SnapshotRead<'a> {
    connection: MutexGuard<'a, Connection>,
    transaction: bool,
}

impl Deref for SnapshotRead<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}

impl Drop for SnapshotRead<'_> {
    fn drop(&mut self) {
        if self.transaction {
            // Read-only transaction; also release its WAL snapshot on errors.
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}

/// Usage reports pair their SQL snapshot with the exact in-memory revision
/// and all-time totals. Persisted-only projections use `SnapshotRead` directly.
pub(super) struct ReportRead<'a> {
    snapshot: SnapshotRead<'a>,
    pub(super) revision: u64,
    pub(super) totals: Option<UsageTotals>,
}

impl Deref for ReportRead<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.snapshot
    }
}

impl TelemetryDb {
    pub(super) fn lock_snapshot_connection(&self) -> Result<SnapshotRead<'_>> {
        let Some(reader) = &self.report_connection else {
            // Private in-memory databases cannot share a second connection.
            return Ok(SnapshotRead {
                connection: self.lock_connection()?,
                transaction: false,
            });
        };
        let connection = reader.lock().map_err(lock_error)?;
        connection
            .execute_batch("BEGIN DEFERRED")
            .map_err(db_error)?;
        Ok(SnapshotRead {
            connection,
            transaction: true,
        })
    }

    pub(super) fn lock_report_connection(&self) -> Result<ReportRead<'_>> {
        let snapshot = self.lock_snapshot_connection()?;
        if !snapshot.transaction {
            return Ok(ReportRead {
                snapshot,
                revision: self.usage_revision.load(Ordering::Acquire),
                totals: self.usage_totals_cache.lock().map_err(lock_error)?.clone(),
            });
        }
        // Only pinning the read snapshot needs the writer lock. Never acquire
        // the reader from the write path (reader -> writer is the lock order).
        let _writer = self.lock_connection()?;
        let mut read = ReportRead {
            snapshot,
            revision: self.usage_revision.load(Ordering::Acquire),
            totals: None,
        };
        read.query_row(
            "SELECT EXISTS(SELECT 1 FROM request_logs LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error)?;
        read.totals = self.usage_totals_cache.lock().map_err(lock_error)?.clone();
        Ok(read)
    }
}

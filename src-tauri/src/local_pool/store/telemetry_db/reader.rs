use super::*;
use std::{ops::Deref, sync::atomic::Ordering, sync::MutexGuard};

/// One bounded reader; WAL lets request recording continue during a report.
/// Keep every query in the report on the same SQLite snapshot and revision.
pub(super) struct ReportRead<'a> {
    connection: MutexGuard<'a, Connection>,
    pub(super) revision: u64,
    pub(super) totals: Option<UsageTotals>,
    transaction: bool,
}

impl Deref for ReportRead<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}

impl Drop for ReportRead<'_> {
    fn drop(&mut self) {
        if self.transaction {
            // Read-only transaction; also release its WAL snapshot on errors.
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}

impl TelemetryDb {
    pub(super) fn lock_report_connection(&self) -> Result<ReportRead<'_>> {
        let Some(reader) = &self.report_connection else {
            // Private in-memory databases cannot share a second connection.
            let connection = self.lock_connection()?;
            return Ok(ReportRead {
                connection,
                revision: self.usage_revision.load(Ordering::Acquire),
                totals: self.usage_totals_cache.lock().map_err(lock_error)?.clone(),
                transaction: false,
            });
        };
        let connection = reader.lock().map_err(lock_error)?;
        // Only pinning the read snapshot needs the writer lock. Never acquire
        // the reader from the write path (reader -> writer is the lock order).
        let _writer = self.lock_connection()?;
        connection
            .execute_batch("BEGIN DEFERRED")
            .map_err(db_error)?;
        let mut read = ReportRead {
            connection,
            revision: self.usage_revision.load(Ordering::Acquire),
            totals: None,
            transaction: true,
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

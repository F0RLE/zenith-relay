use super::*;

fn connection_cache_budget(connection: &Connection) -> Option<usize> {
    let cache_size: i64 = connection
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .ok()?;
    if cache_size < 0 {
        usize::try_from(cache_size.unsigned_abs())
            .ok()?
            .checked_mul(1024)
    } else {
        let page_size: u32 = connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .ok()?;
        usize::try_from(cache_size)
            .ok()?
            .checked_mul(usize::try_from(page_size).ok()?)
    }
}

impl TelemetryDb {
    /// Configured pager budget, not a claim about currently allocated pages.
    pub(crate) fn memory_cache_budget(&self) -> Option<usize> {
        let writer = self.connection.try_lock().ok()?;
        let writer_budget = connection_cache_budget(&writer)?;
        let reader_budget = if let Some(reader) = &self.report_connection {
            let reader = reader.try_lock().ok()?;
            connection_cache_budget(&reader)?
        } else {
            0
        };
        writer_budget.checked_add(reader_budget)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let started = Instant::now();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut connection = Connection::open(path).map_err(db_error)?;
        // Six fixed write-path statements fit here. Do not retain dynamically
        // generated report SQL or grow a cache with the length of the history.
        connection.set_prepared_statement_cache_capacity(8);
        // This file is read by the usage UI while terminal requests append logs.
        // WAL avoids creating and deleting a rollback journal for every request;
        // FULL keeps the same crash durability for the shared local state.
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;\
                 PRAGMA synchronous = FULL;\
                 PRAGMA wal_autocheckpoint = 1000;\
                 PRAGMA cache_size = -32768;\
                 PRAGMA temp_store = MEMORY;\
                 PRAGMA busy_timeout = 5000;",
            )
            .map_err(db_error)?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_error)?;
        if version > LOCAL_DATABASE_SCHEMA_VERSION {
            return Err(LocalPoolError::new(
                ErrorCode::UnsupportedSchema,
                format!(
                    "local database schema {version} is newer than supported schema {LOCAL_DATABASE_SCHEMA_VERSION}"
                ),
            ));
        }
        if version == 0 {
            connection.execute_batch(MIGRATION_001).map_err(db_error)?;
        }
        if version <= 1 {
            connection.execute_batch(MIGRATION_002).map_err(db_error)?;
        }
        if version <= 2 {
            connection.execute_batch(MIGRATION_003).map_err(db_error)?;
        }
        if version <= 3 {
            connection.execute_batch(MIGRATION_004).map_err(db_error)?;
        }
        if version <= 4 {
            connection.execute_batch(MIGRATION_005).map_err(db_error)?;
        }
        if version <= 5 {
            connection.execute_batch(MIGRATION_006).map_err(db_error)?;
        }
        if version <= 6 {
            connection.execute_batch(MIGRATION_007).map_err(db_error)?;
        }
        if version <= 7 {
            connection.execute_batch(MIGRATION_008).map_err(db_error)?;
        }
        if version <= 8 {
            connection.execute_batch(MIGRATION_009).map_err(db_error)?;
        }
        if version <= 9 {
            connection.execute_batch(MIGRATION_010).map_err(db_error)?;
        }
        if version <= 10 {
            connection.execute_batch(MIGRATION_011).map_err(db_error)?;
        }
        if version <= 11 {
            connection.execute_batch(MIGRATION_012).map_err(db_error)?;
        }
        if version <= 12 {
            connection.execute_batch(MIGRATION_013).map_err(db_error)?;
        }
        if version <= 13 {
            connection.execute_batch(MIGRATION_014).map_err(db_error)?;
        }
        if version <= 14 {
            connection.execute_batch(MIGRATION_015).map_err(db_error)?;
        }
        if version <= 15 {
            connection.execute_batch(MIGRATION_016).map_err(db_error)?;
        }
        if version <= 16 {
            connection.execute_batch(MIGRATION_017).map_err(db_error)?;
        }
        if version <= 17 {
            connection.execute_batch(MIGRATION_018).map_err(db_error)?;
        }
        if version <= 18 {
            connection.execute_batch(MIGRATION_019).map_err(db_error)?;
        }
        if version <= 19 {
            connection.execute_batch(MIGRATION_020).map_err(db_error)?;
        }
        if version <= 20 {
            connection.execute_batch(MIGRATION_021).map_err(db_error)?;
        }
        if version <= 21 {
            connection.execute_batch(MIGRATION_022).map_err(db_error)?;
        }
        if version <= 22 {
            connection.execute_batch(MIGRATION_023).map_err(db_error)?;
        }
        if version <= 23 {
            connection.execute_batch(MIGRATION_024).map_err(db_error)?;
        }
        if version <= 24 {
            connection.execute_batch(MIGRATION_025).map_err(db_error)?;
        }
        if version <= 25 {
            connection.execute_batch(MIGRATION_026).map_err(db_error)?;
        }
        if version <= 26 {
            connection.execute_batch(MIGRATION_027).map_err(db_error)?;
        }
        if version <= 27 {
            connection.execute_batch(MIGRATION_028).map_err(db_error)?;
        }
        if version <= 28 {
            connection.execute_batch(MIGRATION_029).map_err(db_error)?;
        }
        if version <= 29 {
            connection.execute_batch(MIGRATION_030).map_err(db_error)?;
        }
        if version <= 30 {
            connection.execute_batch(MIGRATION_031).map_err(db_error)?;
        }
        if version <= 31 {
            migrate_032(&mut connection)?;
        }
        if version <= 32 {
            connection.execute_batch(MIGRATION_033).map_err(db_error)?;
        }
        if version <= 33 {
            migrate_034(&mut connection)?;
        }
        if version <= 34 {
            migrate_035(&mut connection)?;
        }
        if version <= 35 {
            migrate_036(&mut connection)?;
        }
        if version <= 36 {
            connection.execute_batch(MIGRATION_037).map_err(db_error)?;
        }
        if version <= 37 {
            connection.execute_batch(MIGRATION_038).map_err(db_error)?;
        }
        if version <= 38 {
            connection.execute_batch(MIGRATION_039).map_err(db_error)?;
        }
        if version <= 39 {
            connection.execute_batch(MIGRATION_040).map_err(db_error)?;
        }
        if version <= 40 {
            connection.execute_batch(MIGRATION_041).map_err(db_error)?;
        }
        // Retention reads before writing. Acquire the writer reservation first:
        // a finishing background sample on another connection must not turn
        // that read snapshot into an un-upgradable SQLITE_BUSY_SNAPSHOT.
        let retention = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        retention
            .execute_batch(ARCHIVE_USAGE_SQL)
            .map_err(db_error)?;
        retention.commit().map_err(db_error)?;
        let report_connection = connection.path().filter(|path| !path.is_empty()).map(|path| {
            let reader = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(db_error)?;
            reader.set_prepared_statement_cache_capacity(0);
            reader.execute_batch("PRAGMA query_only = ON; PRAGMA cache_size = -24576; PRAGMA temp_store = MEMORY; PRAGMA busy_timeout = 5000;")
                .map_err(db_error)?;
            connection.execute_batch("PRAGMA cache_size = -8192;").map_err(db_error)?;
            Ok::<_, LocalPoolError>(Mutex::new(reader))
        }).transpose()?;
        Ok(Self {
            connection: Mutex::new(connection),
            report_connection,
            usage_revision: AtomicU64::new(0),
            api_equivalent_cache: Mutex::new(None),
            quota_equivalent_cache: Mutex::new(None),
            #[cfg(test)]
            quota_window_reads: AtomicU64::new(0),
            usage_filter_options_cache: Mutex::new(None),
            #[cfg(test)]
            usage_filter_option_reads: AtomicU64::new(0),
            usage_totals_cache: Mutex::new(None),
            usage_report_cache: Mutex::new(None),
            #[cfg(test)]
            usage_report_builds: AtomicU64::new(0),
            open_duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
            quota_samples: Mutex::new(forecast::QuotaSamples::default()),
            capacity_seen: Mutex::new(HashMap::new()),
            capacity_revision: AtomicU64::new(0),
        })
    }

    pub(super) fn cached_usage_totals(
        &self,
        connection: &super::reader::ReportRead<'_>,
        query: &UsageQuery,
        where_sql: &str,
        query_parameters: &[SqlValue],
    ) -> Result<UsageTotals> {
        if is_unfiltered_all_time(query) {
            if let Some(cached) = connection.totals.clone() {
                return Ok(cached);
            }
        }
        let totals = usage_totals(connection, where_sql, query_parameters)?;
        if is_unfiltered_all_time(query) {
            // A concurrent writer may already have advanced its incremental
            // totals. Publication must be fenced with that writer, not just
            // the cache mutex (the revision advances after its cache update).
            let _writer = self
                .report_connection
                .as_ref()
                .map(|_| self.lock_connection())
                .transpose()?;
            if self
                .usage_revision
                .load(std::sync::atomic::Ordering::Acquire)
                == connection.revision
            {
                self.usage_totals_cache
                    .lock()
                    .map_err(lock_error)?
                    .replace(totals.clone());
            }
        }
        Ok(totals)
    }

    pub(super) fn update_cached_usage_totals(
        &self,
        previous_totals: Option<UsageTotals>,
        incoming_totals: UsageTotals,
    ) -> Result<()> {
        let mut cache = self.usage_totals_cache.lock().map_err(lock_error)?;
        let Some(totals) = cache.as_mut() else {
            return Ok(());
        };
        if let Some(previous_totals) = previous_totals {
            apply_usage_totals_delta(totals, previous_totals, false);
        }
        apply_usage_totals_delta(totals, incoming_totals, true);
        Ok(())
    }

    pub(super) fn clear_cached_usage_totals(&self) -> Result<()> {
        self.usage_totals_cache.lock().map_err(lock_error)?.take();
        Ok(())
    }

    pub fn open_duration_ms(&self) -> f64 {
        self.open_duration_ms
    }
}

use super::*;

impl TelemetryDb {
    pub fn open(path: &Path) -> Result<Self> {
        let started = Instant::now();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
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
        connection
            .execute_batch(ARCHIVE_USAGE_SQL)
            .map_err(db_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
            usage_revision: AtomicU64::new(0),
            api_equivalent_cache: Mutex::new(None),
            quota_equivalent_cache: Mutex::new(None),
            usage_totals_cache: Mutex::new(None),
            open_duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
        })
    }

    pub(super) fn cached_usage_totals(
        &self,
        connection: &Connection,
        query: &UsageQuery,
        where_sql: &str,
        values: &[SqlValue],
    ) -> Result<UsageTotals> {
        if is_unfiltered_all_time(query) {
            if let Some(cached) = self
                .usage_totals_cache
                .lock()
                .map_err(lock_error)?
                .as_ref()
                .cloned()
            {
                return Ok(cached);
            }
        }
        let totals = usage_totals(connection, where_sql, values)?;
        if is_unfiltered_all_time(query) {
            self.usage_totals_cache
                .lock()
                .map_err(lock_error)?
                .replace(totals.clone());
        }
        Ok(totals)
    }

    pub(super) fn update_cached_usage_totals(
        &self,
        previous: Option<UsageTotals>,
        current: UsageTotals,
    ) -> Result<()> {
        let mut cache = self.usage_totals_cache.lock().map_err(lock_error)?;
        let Some(totals) = cache.as_mut() else {
            return Ok(());
        };
        if let Some(previous) = previous {
            apply_usage_totals_delta(totals, previous, false);
        }
        apply_usage_totals_delta(totals, current, true);
        Ok(())
    }

    pub(super) fn clear_cached_usage_totals(&self) -> Result<()> {
        self.usage_totals_cache.lock().map_err(lock_error)?.take();
        Ok(())
    }

    pub fn open_duration_ms(&self) -> f64 {
        self.open_duration_ms
    }

    pub fn record_performance(
        &self,
        name: &str,
        duration_ms: f64,
        context: Option<&str>,
    ) -> Result<()> {
        if !valid_performance_name(name)
            || !duration_ms.is_finite()
            || !(0.0..=600_000.0).contains(&duration_ms)
            || context.is_some_and(|value| {
                value.is_empty()
                    || value.len() > 64
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')
                    })
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "performance sample is invalid",
            ));
        }
        self.connection
            .lock()
            .map_err(lock_error)?
            .execute(
                "INSERT INTO performance_samples(name, duration_ms, context) VALUES (?1, ?2, ?3)",
                params![name, duration_ms, context],
            )
            .map_err(db_error)?;
        Ok(())
    }
}

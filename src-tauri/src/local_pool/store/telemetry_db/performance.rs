use super::*;

#[derive(Clone, Copy)]
pub(crate) struct PerformanceSample<'a> {
    pub name: &'a str,
    pub duration_ms: f64,
    pub context: Option<&'a str>,
}

impl PerformanceSample<'_> {
    fn validate(&self) -> Result<()> {
        if !valid_performance_name(self.name)
            || !self.duration_ms.is_finite()
            || !(0.0..=600_000.0).contains(&self.duration_ms)
            || self
                .context
                .is_some_and(|context| !zenith_relay_core::is_ascii_ref(context, 64))
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "performance sample is invalid",
            ));
        }
        Ok(())
    }
}

impl TelemetryDb {
    pub async fn record_performance_async(
        self: &std::sync::Arc<Self>,
        name: String,
        duration_ms: f64,
        context: Option<String>,
    ) -> Result<()> {
        // Page-open timing arrives while Usage may own the same connection.
        // Neither the mutex wait nor SQLite's durable write may run on the UI
        // or async executor thread, even though the renderer does not await it.
        let database = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            database.record_performance(&name, duration_ms, context.as_deref())
        })
        .await
        .map_err(|_| {
            LocalPoolError::new(ErrorCode::Io, "Performance sample write was interrupted")
        })?
    }

    pub fn record_performance(
        &self,
        name: &str,
        duration_ms: f64,
        context: Option<&str>,
    ) -> Result<()> {
        PerformanceSample {
            name,
            duration_ms,
            context,
        }
        .validate()?;
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

    /// Validate the whole snapshot before locking. One transaction preserves
    /// FULL durability without a separate disk commit for every timing stage.
    pub(crate) fn record_performance_batch(&self, samples: &[PerformanceSample<'_>]) -> Result<()> {
        match samples {
            [] => return Ok(()),
            [sample] => {
                return self.record_performance(sample.name, sample.duration_ms, sample.context);
            }
            _ => {}
        }
        for sample in samples {
            sample.validate()?;
        }
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection.transaction().map_err(db_error)?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO performance_samples(name, duration_ms, context) VALUES (?1, ?2, ?3)",
                )
                .map_err(db_error)?;
            for sample in samples {
                statement
                    .execute(params![sample.name, sample.duration_ms, sample.context])
                    .map_err(db_error)?;
            }
        }
        transaction.commit().map_err(db_error)
    }
}

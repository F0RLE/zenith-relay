use super::{db_error, rust_u64, sql_u64, TelemetryDb, MAX_RESPONSE_AFFINITY_ROWS};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use rusqlite::{params, OptionalExtension};
use zenith_relay_core::ResponseAffinityBinding;

impl TelemetryDb {
    pub fn affinity_bindings(&self, now_ms: u64) -> Result<Vec<ResponseAffinityBinding>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        connection
            .execute(
                "DELETE FROM response_affinity WHERE expires_at_ms <= ?1",
                [sql_u64(now_ms)],
            )
            .map_err(db_error)?;
        let mut statement = connection
            .prepare(
                "SELECT response_key, candidate_id, expires_at_ms
                 FROM response_affinity
                 ORDER BY updated_at_ms DESC, response_key DESC
                 LIMIT ?1",
            )
            .map_err(db_error)?;
        let bindings = statement
            .query_map(
                [MAX_RESPONSE_AFFINITY_ROWS as i64],
                affinity_binding_from_row,
            )
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(bindings)
    }

    pub fn find_affinity(&self, key: &str, now_ms: u64) -> Result<Option<ResponseAffinityBinding>> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .query_row(
                "SELECT response_key, candidate_id, expires_at_ms
                 FROM response_affinity WHERE response_key = ?1 AND expires_at_ms > ?2",
                params![key, sql_u64(now_ms)],
                affinity_binding_from_row,
            )
            .optional()
            .map_err(db_error)
    }

    pub fn upsert_affinity(&self, binding: &ResponseAffinityBinding, now_ms: u64) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "INSERT INTO response_affinity(response_key, candidate_id, expires_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(response_key) DO UPDATE SET
                    candidate_id = excluded.candidate_id,
                    expires_at_ms = excluded.expires_at_ms,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    binding.key,
                    binding.candidate_id,
                    sql_u64(binding.expires_at_ms),
                    sql_u64(now_ms),
                ],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn delete_affinity(&self, key: &str) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "DELETE FROM response_affinity WHERE response_key = ?1",
                [key],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn delete_candidate_affinities(&self, candidate_id: &str) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "DELETE FROM response_affinity WHERE candidate_id = ?1",
                [candidate_id],
            )
            .map(|_| ())
            .map_err(db_error)
    }
}

fn affinity_binding_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResponseAffinityBinding> {
    let expires_at_ms: i64 = row.get(2)?;
    Ok(ResponseAffinityBinding {
        key: row.get(0)?,
        candidate_id: row.get(1)?,
        expires_at_ms: rust_u64(expires_at_ms),
    })
}

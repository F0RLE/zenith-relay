use super::{db_error, sql_u64, TelemetryDb, MAX_RESPONSE_AFFINITY_ROWS};
use crate::local_pool::error::Result;
use rusqlite::{params, OptionalExtension};
use zenith_relay_core::{
    ResponseAffinityBinding, RESPONSE_AFFINITY_DELETE_CANDIDATE_SQL,
    RESPONSE_AFFINITY_DELETE_EXPIRED_SQL, RESPONSE_AFFINITY_DELETE_SQL, RESPONSE_AFFINITY_FIND_SQL,
    RESPONSE_AFFINITY_UPSERT_SQL,
};

impl TelemetryDb {
    pub fn affinity_bindings(&self, now_ms: u64) -> Result<Vec<ResponseAffinityBinding>> {
        let connection = self.lock_connection()?;
        connection
            .execute(RESPONSE_AFFINITY_DELETE_EXPIRED_SQL, [sql_u64(now_ms)])
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
        self.lock_connection()?
            .query_row(
                RESPONSE_AFFINITY_FIND_SQL,
                params![key, sql_u64(now_ms)],
                affinity_binding_from_row,
            )
            .optional()
            .map_err(db_error)
    }

    pub fn upsert_affinity(&self, binding: &ResponseAffinityBinding, now_ms: u64) -> Result<()> {
        self.lock_connection()?
            .execute(
                RESPONSE_AFFINITY_UPSERT_SQL,
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
        self.lock_connection()?
            .execute(RESPONSE_AFFINITY_DELETE_SQL, [key])
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn delete_candidate_affinities(&self, candidate_id: &str) -> Result<()> {
        self.lock_connection()?
            .execute(RESPONSE_AFFINITY_DELETE_CANDIDATE_SQL, [candidate_id])
            .map(|_| ())
            .map_err(db_error)
    }
}

fn affinity_binding_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResponseAffinityBinding> {
    Ok(ResponseAffinityBinding::from_stored_expiry(
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
    ))
}

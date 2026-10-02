use super::sqlite::{db_error, unix_time_ms, Store};
use rusqlite::{params, OptionalExtension};
use zenith_relay_core::usage::sql_u64;
use zenith_relay_core::{
    ResponseAffinityBinding, ResponseAffinityStore, RESPONSE_AFFINITY_DELETE_CANDIDATE_SQL,
    RESPONSE_AFFINITY_DELETE_EXPIRED_SQL, RESPONSE_AFFINITY_DELETE_SQL, RESPONSE_AFFINITY_FIND_SQL,
    RESPONSE_AFFINITY_UPSERT_SQL,
};

impl ResponseAffinityStore for Store {
    fn load(&self, now_ms: u64) -> Result<Vec<ResponseAffinityBinding>, String> {
        let connection = self.lock()?;
        connection
            .execute(RESPONSE_AFFINITY_DELETE_EXPIRED_SQL, [sql_u64(now_ms)])
            .map_err(db_error)?;
        let mut statement = connection
            .prepare(
                "SELECT response_key, candidate_id, expires_at_ms
                 FROM response_affinity ORDER BY updated_at_ms DESC",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], response_affinity_from_row)
            .map_err(db_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)
    }

    fn find(&self, key: &str, now_ms: u64) -> Result<Option<ResponseAffinityBinding>, String> {
        self.lock()?
            .query_row(
                RESPONSE_AFFINITY_FIND_SQL,
                params![key, sql_u64(now_ms)],
                response_affinity_from_row,
            )
            .optional()
            .map_err(db_error)
    }

    fn upsert(&self, binding: &ResponseAffinityBinding) -> Result<(), String> {
        self.lock()?
            .execute(
                RESPONSE_AFFINITY_UPSERT_SQL,
                params![
                    binding.key,
                    binding.candidate_id,
                    sql_u64(binding.expires_at_ms),
                    sql_u64(unix_time_ms()),
                ],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.lock()?
            .execute(RESPONSE_AFFINITY_DELETE_SQL, [key])
            .map(|_| ())
            .map_err(db_error)
    }

    fn delete_candidate(&self, candidate_id: &str) -> Result<(), String> {
        self.lock()?
            .execute(RESPONSE_AFFINITY_DELETE_CANDIDATE_SQL, [candidate_id])
            .map(|_| ())
            .map_err(db_error)
    }
}

fn response_affinity_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ResponseAffinityBinding> {
    Ok(ResponseAffinityBinding::from_stored_expiry(
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
    ))
}

use super::sqlite::{db_error, optional_u64, unix_time_ms, Store};
use rusqlite::{params, OptionalExtension};
use zenith_relay_core::{ResponseAffinityBinding, ResponseAffinityStore};

impl ResponseAffinityStore for Store {
    fn load(&self, now_ms: u64) -> Result<Vec<ResponseAffinityBinding>, String> {
        let connection = self.lock()?;
        connection
            .execute(
                "DELETE FROM response_affinity WHERE expires_at_ms <= ?1",
                [sql_u64(now_ms)],
            )
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
                "SELECT response_key, candidate_id, expires_at_ms
                 FROM response_affinity WHERE response_key = ?1 AND expires_at_ms > ?2",
                params![key, sql_u64(now_ms)],
                response_affinity_from_row,
            )
            .optional()
            .map_err(db_error)
    }

    fn upsert(&self, binding: &ResponseAffinityBinding) -> Result<(), String> {
        self.lock()?
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
                    sql_u64(unix_time_ms()),
                ],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "DELETE FROM response_affinity WHERE response_key = ?1",
                [key],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    fn delete_candidate(&self, candidate_id: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "DELETE FROM response_affinity WHERE candidate_id = ?1",
                [candidate_id],
            )
            .map(|_| ())
            .map_err(db_error)
    }
}

fn response_affinity_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ResponseAffinityBinding> {
    Ok(ResponseAffinityBinding {
        key: row.get(0)?,
        candidate_id: row.get(1)?,
        expires_at_ms: optional_u64(Some(row.get(2)?)).unwrap_or_default(),
    })
}

fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

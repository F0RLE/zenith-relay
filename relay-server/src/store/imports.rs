use super::sqlite::{db_error, Store};
use rusqlite::{params, OptionalExtension};

#[derive(Clone, Debug)]
pub struct PendingImport {
    pub id: String,
    pub preview_json: String,
    pub secret_ref: String,
    pub created_at_ms: u64,
}

impl Store {
    pub fn save_pending_import(&self, import: &PendingImport) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT INTO pending_imports(id, preview_json, secret_ref, created_at_ms) VALUES (?1, ?2, ?3, ?4)\
                 ON CONFLICT(id) DO UPDATE SET preview_json=excluded.preview_json, secret_ref=excluded.secret_ref, created_at_ms=excluded.created_at_ms",
                params![
                    import.id,
                    import.preview_json,
                    import.secret_ref,
                    import.created_at_ms.min(i64::MAX as u64) as i64
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn pending_import(&self, id: &str) -> Result<Option<PendingImport>, String> {
        self.lock()?
            .query_row(
                "SELECT id, preview_json, secret_ref, created_at_ms FROM pending_imports WHERE id = ?1",
                [id],
                |row| {
                    Ok(PendingImport {
                        id: row.get(0)?,
                        preview_json: row.get(1)?,
                        secret_ref: row.get(2)?,
                        created_at_ms: row.get::<_, i64>(3)?.max(0) as u64,
                    })
                },
            )
            .optional()
            .map_err(db_error)
    }

    pub fn delete_pending_import(&self, id: &str) -> Result<bool, String> {
        Ok(self
            .lock()?
            .execute("DELETE FROM pending_imports WHERE id = ?1", [id])
            .map_err(db_error)?
            > 0)
    }

    pub fn delete_pending_imports_before(&self, cutoff_ms: u64) -> Result<Vec<String>, String> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(db_error)?;
        let secret_refs = {
            let mut statement = transaction
                .prepare("SELECT secret_ref FROM pending_imports WHERE created_at_ms < ?1")
                .map_err(db_error)?;
            let rows = statement
                .query_map([cutoff_ms.min(i64::MAX as u64) as i64], |row| row.get(0))
                .map_err(db_error)?
                .collect::<Result<Vec<String>, _>>()
                .map_err(db_error)?;
            rows
        };
        transaction
            .execute(
                "DELETE FROM pending_imports WHERE created_at_ms < ?1",
                [cutoff_ms.min(i64::MAX as u64) as i64],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(secret_refs)
    }
}

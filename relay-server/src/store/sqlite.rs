use super::migrations::{
    apply_migrations, finish_migration, prepare_migration_backup, read_schema_version,
    recover_interrupted_migration, sibling_path, validate_migration_ledger,
};
use crate::state::SERVER_SCHEMA_VERSION;
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let migration_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(sibling_path(&path, ".migration.lock"))
            .map_err(io_error)?;
        migration_lock
            .try_lock_exclusive()
            .map_err(|_| "database migration is already running".to_string())?;
        recover_interrupted_migration(&path)?;
        let database_existed = path.metadata().is_ok_and(|metadata| metadata.len() > 0);
        let mut connection = Connection::open(&path).map_err(db_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        let current_version = read_schema_version(&connection)?;
        if current_version > SERVER_SCHEMA_VERSION {
            return Err(format!(
                "database schema version {current_version} is newer than supported version {SERVER_SCHEMA_VERSION}"
            ));
        }
        if current_version < SERVER_SCHEMA_VERSION {
            if database_existed {
                prepare_migration_backup(
                    &connection,
                    &path,
                    current_version,
                    SERVER_SCHEMA_VERSION,
                )?;
            }
            apply_migrations(&mut connection, current_version)?;
            if database_existed {
                finish_migration(&path)?;
            }
        }
        validate_migration_ledger(&connection)?;
        drop(migration_lock);
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(db_error)?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        let _ = store.server_id()?;
        Ok(store)
    }

    pub fn server_id(&self) -> Result<String, String> {
        if let Some(value) = self.metadata("server_id")? {
            return Ok(value);
        }
        let value = uuid::Uuid::new_v4().to_string();
        self.set_metadata("server_id", &value)?;
        Ok(value)
    }

    pub(super) fn metadata(&self, key: &str) -> Result<Option<String>, String> {
        self.lock()?
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)
    }

    pub(super) fn set_metadata(&self, key: &str, value: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub(super) fn list_records<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>, String> {
        let sql = format!("SELECT data_json FROM {table} ORDER BY id");
        let connection = self.lock()?;
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| parse_json(&row.map_err(db_error)?))
            .collect()
    }

    pub(super) fn save_record<T: Serialize>(
        &self,
        table: &str,
        id: &str,
        secret_ref: &str,
        value: &T,
    ) -> Result<(), String> {
        let sql = format!(
            "INSERT INTO {table}(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref"
        );
        self.lock()?
            .execute(&sql, params![id, to_json(value)?, secret_ref])
            .map_err(db_error)?;
        Ok(())
    }

    pub(super) fn delete_record<T: DeserializeOwned>(
        &self,
        table: &str,
        id: &str,
    ) -> Result<Option<T>, String> {
        let sql = format!("SELECT data_json FROM {table} WHERE id = ?1");
        let delete = format!("DELETE FROM {table} WHERE id = ?1");
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(db_error)?;
        let json = transaction
            .query_row(&sql, [id], |row| row.get::<_, String>(0))
            .optional()
            .map_err(db_error)?;
        if json.is_some() {
            transaction.execute(&delete, [id]).map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        json.map(|value| parse_json(&value)).transpose()
    }

    pub(super) fn lock(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "SQLite lock poisoned".to_string())
    }
}

pub(super) fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

pub(super) fn to_json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "record serialization failed".to_string())
}

pub(super) fn parse_json<T: DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|_| "stored record is invalid".to_string())
}

pub(super) fn optional_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

pub(super) fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

pub(super) fn io_error(error: std::io::Error) -> String {
    format!("store I/O failed: {error}")
}

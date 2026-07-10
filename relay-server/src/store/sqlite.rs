use crate::state::{GatewayKeyRecord, ServerAccountRecord, SourceRecord};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use zenith_relay_core::{
    automations::{WakeAutomationState, WakeTask},
    protocol::{UsagePage, UsageSummary},
    UsageEvent, WireApi,
};

const MIGRATION: &str = include_str!("../../migrations/001_init.sql");

#[derive(Clone, Debug)]
pub struct PendingImport {
    pub id: String,
    pub preview_json: String,
    pub secret_ref: String,
    pub created_at_ms: u64,
}

pub struct Store {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(&path).map_err(db_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(db_error)?;
        connection.execute_batch(MIGRATION).map_err(db_error)?;
        let store = Self {
            path,
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

    pub fn gateway_enabled(&self) -> Result<bool, String> {
        Ok(self
            .metadata("gateway_enabled")?
            .is_none_or(|value| value == "true"))
    }

    pub fn set_gateway_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata("gateway_enabled", if enabled { "true" } else { "false" })
    }

    pub fn sources(&self) -> Result<Vec<SourceRecord>, String> {
        self.list_records("sources")
    }

    pub fn save_source(&self, record: &SourceRecord) -> Result<(), String> {
        self.save_record("sources", &record.id, &record.secret_ref, record)
    }

    pub fn delete_source(&self, id: &str) -> Result<Option<SourceRecord>, String> {
        self.delete_record("sources", id)
    }

    pub fn accounts(&self) -> Result<Vec<ServerAccountRecord>, String> {
        self.list_records("accounts")
    }

    pub fn save_account(&self, record: &ServerAccountRecord) -> Result<(), String> {
        self.save_record("accounts", &record.id, &record.secret_ref, record)
    }

    pub fn delete_account(&self, id: &str) -> Result<Option<ServerAccountRecord>, String> {
        self.delete_record("accounts", id)
    }

    pub fn keys(&self) -> Result<Vec<GatewayKeyRecord>, String> {
        self.list_records("gateway_keys")
    }

    pub fn save_key(&self, record: &GatewayKeyRecord) -> Result<(), String> {
        self.save_record("gateway_keys", &record.id, &record.secret_ref, record)
    }

    pub fn delete_key(&self, id: &str) -> Result<Option<GatewayKeyRecord>, String> {
        self.delete_record("gateway_keys", id)
    }

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

    pub fn wake_tasks(&self) -> Result<Vec<WakeTask>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT data_json FROM wake_tasks ORDER BY id")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| parse_json(&row.map_err(db_error)?))
            .collect()
    }

    pub fn save_wake_task(&self, task: &WakeTask) -> Result<(), String> {
        let json = to_json(task)?;
        self.lock()?
            .execute(
                "INSERT INTO wake_tasks(id, data_json) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json",
                params![task.id, json],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn delete_wake_task(&self, id: &str) -> Result<bool, String> {
        Ok(self
            .lock()?
            .execute("DELETE FROM wake_tasks WHERE id = ?1", [id])
            .map_err(db_error)?
            > 0)
    }

    pub fn wake_state(&self) -> Result<WakeAutomationState, String> {
        let json = self
            .lock()?
            .query_row(
                "SELECT data_json FROM wake_state WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?;
        match json {
            Some(value) => parse_json(&value),
            None => WakeAutomationState::new(1_024, 256).map_err(|error| error.to_string()),
        }
    }

    pub fn save_wake_state(&self, state: &WakeAutomationState) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT INTO wake_state(singleton, data_json) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET data_json=excluded.data_json",
                [to_json(state)?],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn record_usage(&self, event: &UsageEvent, created_at_ms: u64) -> Result<(), String> {
        let candidate_id = event
            .account_id
            .as_deref()
            .or(event.candidate_id.as_deref())
            .unwrap_or(&event.source_id);
        let candidate_kind = if event.account_id.is_some() {
            "account"
        } else {
            "source"
        };
        let candidate_hint =
            format!("{:x}", Sha256::digest(candidate_id.as_bytes()))[..12].to_string();
        self.lock()?
            .execute(
                "INSERT INTO usage_events(request_id, local_key_id, candidate_kind, candidate_hint, requested_model, resolved_model, wire_api, success, http_status, error_category, latency_ms, input_tokens, output_tokens, total_tokens, created_at_ms)\
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    event.request_id,
                    event.local_key_id,
                    candidate_kind,
                    candidate_hint,
                    event.requested_model,
                    event.resolved_model,
                    wire_api_name(event.wire_api),
                    i64::from(event.success),
                    i64::from(event.http_status),
                    event.error_category,
                    event.latency_ms as i64,
                    event.input_tokens.map(|value| value as i64),
                    event.output_tokens.map(|value| value as i64),
                    event.total_tokens.map(|value| value as i64),
                    created_at_ms as i64,
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn usage_page(&self, page: u32, page_size: u32) -> Result<UsagePage, String> {
        let page = page.max(1);
        let page_size = page_size.clamp(1, 200);
        let connection = self.lock()?;
        let total = connection
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(db_error)?
            .max(0) as u64;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let mut statement = connection
            .prepare(
                "SELECT id, request_id, local_key_id, candidate_kind, candidate_hint, requested_model, resolved_model, wire_api, success, http_status, error_category, latency_ms, input_tokens, output_tokens, total_tokens, created_at_ms\
                 FROM usage_events ORDER BY id DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![i64::from(page_size), offset as i64], |row| {
                let wire_api: String = row.get(7)?;
                Ok(UsageSummary {
                    id: row.get(0)?,
                    request_id: row.get(1)?,
                    local_key_id: row.get(2)?,
                    candidate_kind: row.get(3)?,
                    candidate_hint: row.get(4)?,
                    requested_model: row.get(5)?,
                    resolved_model: row.get(6)?,
                    wire_api: parse_wire_api(&wire_api),
                    success: row.get::<_, i64>(8)? != 0,
                    http_status: row.get::<_, i64>(9)?.clamp(0, i64::from(u16::MAX)) as u16,
                    error_category: row.get(10)?,
                    latency_ms: row.get::<_, i64>(11)?.max(0) as u64,
                    input_tokens: optional_u64(row.get(12)?),
                    output_tokens: optional_u64(row.get(13)?),
                    total_tokens: optional_u64(row.get(14)?),
                    created_at_ms: row.get::<_, i64>(15)?.max(0) as u64,
                })
            })
            .map_err(db_error)?;
        let events = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(u64::from(page_size)) as u32
        };
        Ok(UsagePage {
            events,
            total,
            page,
            page_size,
            total_pages,
        })
    }

    pub fn backup_to(&self, destination: &Path) -> Result<(), String> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        self.lock()?
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(db_error)?;
        fs::copy(&self.path, destination).map_err(io_error)?;
        Ok(())
    }

    fn metadata(&self, key: &str) -> Result<Option<String>, String> {
        self.lock()?
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)
    }

    fn set_metadata(&self, key: &str, value: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn list_records<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>, String> {
        let sql = format!("SELECT data_json FROM {table} ORDER BY id");
        let connection = self.lock()?;
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| parse_json(&row.map_err(db_error)?))
            .collect()
    }

    fn save_record<T: Serialize>(
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

    fn delete_record<T: DeserializeOwned>(
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

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "SQLite lock poisoned".to_string())
    }
}

fn to_json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "record serialization failed".to_string())
}

fn parse_json<T: DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|_| "stored record is invalid".to_string())
}

fn optional_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn wire_api_name(value: WireApi) -> &'static str {
    match value {
        WireApi::Responses => "responses",
        WireApi::ChatCompletions => "chat_completions",
        WireApi::Messages => "messages",
    }
}

fn parse_wire_api(value: &str) -> WireApi {
    match value {
        "chat_completions" => WireApi::ChatCompletions,
        "messages" => WireApi::Messages,
        _ => WireApi::Responses,
    }
}

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

fn io_error(error: std::io::Error) -> String {
    format!("store I/O failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_migrates_and_preserves_server_identity() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-store-{}", uuid::Uuid::new_v4()));
        let path = root.join("relay.sqlite");
        let first = Store::open(path.clone()).unwrap();
        let server_id = first.server_id().unwrap();
        assert!(first.gateway_enabled().unwrap());
        drop(first);
        let second = Store::open(path).unwrap();
        assert_eq!(second.server_id().unwrap(), server_id);
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }
}
